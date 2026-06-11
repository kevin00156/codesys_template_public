#!/usr/bin/env python3
"""
fetch_all.py — Download all items listed in a skill's index.json
                and write `reference/<Name>.{md,json}` for each.

Library-agnostic; takes the skill directory as an argument.

Usage:
    python fetch_all.py <skill_dir>
    python fetch_all.py <skill_dir> --limit 30
    python fetch_all.py <skill_dir> --force
    python fetch_all.py <skill_dir> --concurrency 6

The skill directory must contain `index.json` (created by build_index.py).
Output:
    <skill_dir>/reference/<Name>.md
    <skill_dir>/reference/<Name>.json
    <skill_dir>/fetch_log.json
"""
import argparse
import json
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

try:
    import requests
except ImportError:
    print("錯誤：請先安裝 requests：pip install requests")
    sys.exit(1)

sys.path.insert(0, str(Path(__file__).parent))
from extract import parse_definition  # noqa: E402


HEADERS = {"User-Agent": "lib-skill-builder/1.0"}


def html_to_markdown_simple(html: str) -> str:
    """HTML → markdown for extract.py to parse. Prefer html2text; fall back to BS4."""
    try:
        import html2text
        h = html2text.HTML2Text()
        h.body_width = 0
        h.ignore_images = False
        h.ignore_links = False
        return h.handle(html)
    except ImportError:
        pass

    try:
        from bs4 import BeautifulSoup
    except ImportError:
        raise RuntimeError("Need html2text or beautifulsoup4")

    soup = BeautifulSoup(html, "html.parser")
    main = soup.find("div", attrs={"role": "main"}) or soup.find("div", class_="document") or soup.body
    if not main:
        return ""

    out: list[str] = []
    for el in main.find_all(["h1", "h2", "h3", "p", "table", "div"]):
        if el.name == "h1":
            out.append(f"# {el.get_text(strip=True)}[¶]")
        elif el.name == "h2":
            out.append(f"## {el.get_text(strip=True)}")
        elif el.name == "p":
            out.append(el.get_text(" ", strip=True))
        elif el.name == "table":
            rows = []
            for tr in el.find_all("tr"):
                cells = [td.get_text(" ", strip=True) for td in tr.find_all(["th", "td"])]
                if cells:
                    rows.append("| " + " | ".join(cells) + " |")
            if rows:
                header = rows[0]
                sep = "| " + " | ".join(["---"] * header.count("|")) + " |"
                rows.insert(1, sep)
                out.append("InOut:\n:   " + "\n    ".join(rows))
        out.append("")
    return "\n".join(out)


def fetch_one(item: dict, ref_dir: Path, session: requests.Session, force: bool) -> dict:
    name = item["name"]
    url = item["doc_url"]
    type_code = item.get("type_code", "FN")

    md_path = ref_dir / f"{name}.md"
    json_path = ref_dir / f"{name}.json"

    if not force and md_path.exists() and json_path.exists():
        return {"name": name, "status": "skipped", "url": url}

    try:
        r = session.get(url, headers=HEADERS, timeout=30)
        r.raise_for_status()
        markdown = html_to_markdown_simple(r.text)

        parsed = parse_definition(markdown, name, type_code)
        if not parsed.get("extracted"):
            return {"name": name, "status": "parse_failed", "url": url, "error": parsed.get("error")}

        md_path.write_text(parsed["raw_content"], encoding="utf-8")
        meta = {k: v for k, v in parsed.items() if k != "raw_content"}
        meta["category"] = item.get("category", "")
        meta["subcategory"] = item.get("subcategory", "")
        meta["doc_url"] = url
        meta["type"] = item.get("type", type_code)
        json_path.write_text(json.dumps(meta, ensure_ascii=False, indent=2), encoding="utf-8")

        return {"name": name, "status": "ok", "url": url}
    except requests.RequestException as e:
        return {"name": name, "status": "http_error", "url": url, "error": str(e)}
    except Exception as e:
        return {"name": name, "status": "error", "url": url, "error": str(e)}


def main():
    p = argparse.ArgumentParser(description="Fetch all items from a skill's index.json")
    p.add_argument("skill_dir", help="Path to skill dir containing index.json")
    p.add_argument("--limit", type=int, default=0, help="Limit count (0 = all)")
    p.add_argument("--force", action="store_true", help="Force re-download")
    p.add_argument("--concurrency", type=int, default=4)
    args = p.parse_args()

    skill_dir = Path(args.skill_dir).resolve()
    index_path = skill_dir / "index.json"
    ref_dir = skill_dir / "reference"
    log_path = skill_dir / "fetch_log.json"

    if not index_path.exists():
        print(f"錯誤：找不到索引檔 {index_path}")
        sys.exit(1)

    ref_dir.mkdir(exist_ok=True)
    data = json.loads(index_path.read_text(encoding="utf-8"))
    items = data["items"]
    if args.limit:
        items = items[: args.limit]

    print(f"準備抓取 {len(items)} 個項目，並行 {args.concurrency}")
    print(f"輸出目錄：{ref_dir}")

    session = requests.Session()
    results: list[dict] = []
    start = time.time()

    with ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        futures = {pool.submit(fetch_one, it, ref_dir, session, args.force): it for it in items}
        for i, fut in enumerate(as_completed(futures), 1):
            res = fut.result()
            results.append(res)
            mark = {
                "ok":            "OK",
                "skipped":       "..",
                "parse_failed":  "??",
                "http_error":    "XX",
                "error":         "XX",
            }.get(res["status"], "?")
            line = f"  [{i:3}/{len(items)}] {mark} {res['name']:45} {res['status']}"
            if "error" in res:
                line += f" - {res.get('error', '')[:60]}"
            print(line)

    elapsed = time.time() - start
    by_status: dict[str, int] = {}
    for r in results:
        by_status[r["status"]] = by_status.get(r["status"], 0) + 1

    print(f"\n完成！耗時 {elapsed:.1f} 秒")
    for s, n in sorted(by_status.items(), key=lambda x: -x[1]):
        print(f"  {s}: {n}")

    log_path.write_text(
        json.dumps({"elapsed_sec": round(elapsed, 1), "results": results}, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    print(f"\n詳細記錄：{log_path}")


if __name__ == "__main__":
    main()
