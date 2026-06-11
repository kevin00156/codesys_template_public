#!/usr/bin/env python3
"""
build_index.py — Crawl a CODESYS library doc root to produce index.json.

The CODESYS docs on `content.helpme-codesys.com` are Sphinx-generated. Every page
has a left-side toctree linking to every other page in the same library. So we:

  1. Fetch the doc root once.
  2. Collect every `<a href="...">` that points to a `.html` page under the same
     URL prefix.
  3. Visit each candidate. If its `<h1>` matches `Name (TYPE)`, treat it as a
     leaf item and record { name, type_code, category, doc_url }.
  4. Write items sorted by (category, subcategory, name) to <skill_dir>/index.json.

Usage:
    python build_index.py <doc_root_url> <skill_dir>

Example:
    python build_index.py \\
        https://content.helpme-codesys.com/en/libs/Standard/Current/ \\
        ../../.claude/skills/standard-reference
"""
import argparse
import json
import re
import sys
import time
from pathlib import Path
from urllib.parse import urljoin, urlparse, unquote
from concurrent.futures import ThreadPoolExecutor, as_completed

try:
    import requests
    from bs4 import BeautifulSoup
except ImportError as e:
    print(f"Missing dependency: {e}. Run: pip install -r requirements.txt")
    sys.exit(1)


HEADERS = {"User-Agent": "lib-skill-builder/1.0"}

# Type-label-in-heading → short type_code used by extract.py
TYPE_CODE_MAP = {
    "FB":             "FB",
    "FUNCTION_BLOCK": "FB",
    "FUN":            "FN",
    "Function":       "FN",
    "STRUCT":         "ST",
    "Struct":         "ST",
    "ENUM":           "EN",
    "Enum":           "EN",
    "Alias":          "AL",
    "ALIAS":          "AL",
    "ITF":            "IF",
    "Interface":      "IF",
    "GVL":            "GVL",
    "ParamList":      "PL",
    "ImagePool":      "IM",
    "GlobalTextList": "GTL",
    "Method":         "MT",
    "METH":           "MT",
    "Property":       "PR",
    "PROP":           "PR",
    "Action":         "AC",
    "ACT":            "AC",
}

TYPE_TW = {
    "FB":  "功能塊 (FunctionBlock)",
    "FN":  "函式 (Function)",
    "ST":  "結構 (Struct)",
    "EN":  "列舉 (Enum)",
    "AL":  "別名 (Alias)",
    "IF":  "介面 (Interface)",
    "GVL": "全域變數列表 (GVL)",
    "PL":  "參數列表 (ParamList)",
    "IM":  "影像池 (ImagePool)",
    "GTL": "全域文字列表 (GlobalTextList)",
    "MT":  "方法 (Method)",
    "PR":  "屬性 (Property)",
    "AC":  "動作 (Action)",
}

TYPE_NAME = {
    "FB":  "FunctionBlock",
    "FN":  "Function",
    "ST":  "Struct",
    "EN":  "Enum",
    "AL":  "Alias",
    "IF":  "Interface",
    "GVL": "GVL",
    "PL":  "ParamList",
    "IM":  "ImagePool",
    "GTL": "GlobalTextList",
    "MT":  "Method",
    "PR":  "Property",
    "AC":  "Action",
}


def fetch(url: str, session: requests.Session) -> str:
    r = session.get(url, headers=HEADERS, timeout=30)
    r.raise_for_status()
    return r.text


def parse_h1_for_type(html: str) -> tuple[str | None, str | None]:
    """Return (name, type_code) parsed from the H1 of a leaf doc page, else (None, None).
    Name may include dots for methods/properties (e.g. `CamBuilder.Append`)."""
    soup = BeautifulSoup(html, "html.parser")
    h1 = soup.find("h1")
    if not h1:
        return None, None
    text = h1.get_text(separator=" ", strip=False)
    text = text.replace("\xa0", " ").replace("¶", " ").strip()
    # CODESYS H1 format: "Name (TYPE)" or "Parent.Method (TYPE)"
    m = re.match(r"([A-Za-z_][\w]*(?:\.[A-Za-z_][\w]*)*)\s*\(([^)]+)\)", text)
    if not m:
        return None, None
    name = m.group(1)
    type_label = m.group(2).strip()
    type_code = TYPE_CODE_MAP.get(type_label)
    return name, type_code


def discover_links(html: str, base_url: str, doc_root_url: str) -> set[str]:
    """Find all `<a href>` links from a page that are .html under the lib's URL prefix."""
    soup = BeautifulSoup(html, "html.parser")
    base_path = urlparse(doc_root_url).path
    prefix = base_path.rsplit("/", 1)[0] + "/"

    links: set[str] = set()
    for a in soup.find_all("a", href=True):
        href = a["href"]
        if not href or href.startswith("#") or href.startswith("mailto:"):
            continue
        abs_url = urljoin(base_url, href).split("#")[0]
        parsed = urlparse(abs_url)
        if parsed.scheme not in ("http", "https"):
            continue
        if parsed.netloc != "content.helpme-codesys.com":
            continue
        if not parsed.path.endswith(".html"):
            continue
        if not parsed.path.startswith(prefix):
            continue
        # Skip the doc root itself
        if parsed.path == base_path or parsed.path.rstrip("/") == base_path.rstrip("/"):
            continue
        links.add(abs_url)
    return links


def url_to_category(url: str, doc_root_url: str) -> tuple[str, str]:
    """Path between the doc root and the leaf filename → (category, subcategory).
    Drops `pou-<X>/` segments — those are method containers, not real categories."""
    base_path = urlparse(doc_root_url).path
    prefix = base_path.rsplit("/", 1)[0] + "/"
    full_path = urlparse(url).path
    if not full_path.startswith(prefix):
        return "", ""
    rel = full_path[len(prefix):]
    raw_parts = rel.split("/")[:-1]  # drop the trailing <Name>.html
    parts = []
    for p in raw_parts:
        u = unquote(p)
        if u.startswith("pou-"):
            continue  # method container, not a real category
        parts.append(u.replace("-", " ").replace("_", " "))
    if not parts:
        return "", ""
    return parts[0], "/".join(parts[1:])


def parent_from_url(url: str) -> str | None:
    """For URLs like `.../pou-<Parent>/<Name>.html`, return `<Parent>`. Else None."""
    path = urlparse(url).path
    parts = path.split("/")
    for p in parts[:-1]:
        u = unquote(p)
        if u.startswith("pou-"):
            return u[4:]
    return None


def process_one(url: str, session: requests.Session, doc_root_url: str) -> dict | None:
    try:
        html = fetch(url, session)
    except requests.RequestException as e:
        return {"_error": f"{url}: {e}"}
    name, type_code = parse_h1_for_type(html)
    if not name or not type_code:
        return None  # not a leaf item page
    # For methods/properties/actions, H1 usually already includes parent (e.g. CamBuilder.Append).
    # If not (older doc format), fall back to URL-derived parent.
    if type_code in ("MT", "PR", "AC") and "." not in name:
        parent = parent_from_url(url)
        if parent:
            name = f"{parent}.{name}"
    category, subcategory = url_to_category(url, doc_root_url)
    return {
        "category": category,
        "subcategory": subcategory,
        "name": name,
        "type_code": type_code,
        "type": TYPE_NAME.get(type_code, type_code),
        "type_tw": TYPE_TW.get(type_code, type_code),
        "doc_url": url,
    }


def crawl(doc_root_url: str, concurrency: int = 6) -> list[dict]:
    session = requests.Session()
    print(f"Fetching doc root: {doc_root_url}")
    root_html = fetch(doc_root_url, session)
    candidates = discover_links(root_html, doc_root_url, doc_root_url)
    print(f"Found {len(candidates)} candidate pages from root")

    items: list[dict] = []
    errors: list[str] = []
    start = time.time()

    with ThreadPoolExecutor(max_workers=concurrency) as pool:
        futures = {pool.submit(process_one, url, session, doc_root_url): url for url in sorted(candidates)}
        for i, fut in enumerate(as_completed(futures), 1):
            res = fut.result()
            if res is None:
                continue
            if "_error" in res:
                errors.append(res["_error"])
                continue
            items.append(res)
            if i % 25 == 0 or i == len(candidates):
                print(f"  [{i:4}/{len(candidates)}] {len(items)} items so far")

    elapsed = time.time() - start
    print(f"Done in {elapsed:.1f}s. {len(items)} items, {len(errors)} errors.")
    for e in errors[:5]:
        print(f"  WARN: {e}")
    if len(errors) > 5:
        print(f"  ...and {len(errors) - 5} more")
    return items


def main():
    p = argparse.ArgumentParser(description="Crawl a CODESYS library doc root to build index.json")
    p.add_argument("doc_root_url", help="e.g. https://content.helpme-codesys.com/en/libs/Standard/Current/")
    p.add_argument("skill_dir", help="Skill output dir (index.json will be written into it)")
    p.add_argument("--concurrency", type=int, default=6)
    args = p.parse_args()

    if not args.doc_root_url.endswith("/"):
        args.doc_root_url += "/"

    out_dir = Path(args.skill_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    items = crawl(args.doc_root_url, concurrency=args.concurrency)
    items.sort(key=lambda x: (x["category"], x["subcategory"], x["name"]))

    # Derive a lib display name from the URL
    parts = [p for p in urlparse(args.doc_root_url).path.strip("/").split("/") if p]
    lib_display = unquote(parts[-2]) if len(parts) >= 2 else "Unknown"

    index = {
        "library": lib_display,
        "doc_root": args.doc_root_url.rstrip("/"),
        "total_items": len(items),
        "items": items,
    }

    out_path = out_dir / "index.json"
    out_path.write_text(json.dumps(index, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"\nWrote {out_path} ({len(items)} items)")


if __name__ == "__main__":
    main()
