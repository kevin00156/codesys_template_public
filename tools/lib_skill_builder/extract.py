#!/usr/bin/env python3
"""
從 CODESYS 文件頁的 markdown 輸出中抽出實際定義內容。

每個頁面結構：
  [大量 nav tree]
  [breadcrumb]
  ---
  # {Name} ({TypeAbbr})[¶]...
  
  FUNCTION_BLOCK / STRUCT / TYPE ...
  
  [描述]
  
  [Example 區塊（可選）]
  
  InOut: / Inputs / Outputs 表格（FB/Function）
  或：結構成員表 / 列舉值表
"""
import re
import json
import sys
from pathlib import Path


# 型別代碼 → 頁面標題中使用的縮寫
TYPE_TITLE_MAP = {
    "FB":  ["FB", "FUNCTION_BLOCK"],
    "ST":  ["STRUCT", "Struct"],
    "EN":  ["ENUM", "Enum"],
    "AL":  ["Alias", "ALIAS"],
    "FN":  ["FUN", "Function"],
    "IF":  ["ITF", "Interface"],
    "GVL": ["GVL"],
    "PL":  ["ParamList"],
    "IM":  ["ImagePool"],
    "GTL": ["GlobalTextList"],
}


def extract_content(markdown: str, name: str) -> str | None:
    """
    從整頁 markdown 中找到 `# {name} (...)` 區段並回傳之後的內容。
    name 中的底線在 markdown 中可能被跳脫為 \\_，需處理。
    """
    # 把 name 中的 _ 變成可同時匹配 _ 或 \_ 的 regex
    # re.escape 不會轉義底線，所以直接把字面 _ 換成 \\?_
    name_pattern = re.escape(name).replace("_", r"\\?_")
    
    # 在「文件 nav」之後才會出現的標題：以 # 開頭、後面接縮寫
    pattern = re.compile(
        rf"^# {name_pattern}\s*\([^)]+\)\[¶\].*?$",
        re.MULTILINE,
    )
    
    matches = list(pattern.finditer(markdown))
    if not matches:
        # 備援：忽略括號內容
        pattern2 = re.compile(
            rf"^# {name_pattern}\s*\(",
            re.MULTILINE,
        )
        matches = list(pattern2.finditer(markdown))
        if not matches:
            return None
    
    # 取最後一個（避免抓到 nav 中可能誤判的）
    start = matches[-1].start()
    content = markdown[start:].rstrip()
    
    # 收尾：移除可能殘留的下一頁 nav，截斷在已知終止標記
    # 此文件結構通常以表格結尾或新的 nav block 開始
    return content


def parse_io_table(content: str) -> dict:
    """
    從 FB/Function/Method/Property/Action 的 InOut 段落解析出 inputs/outputs/inouts/return。
    支援三種表頭：
      - 5 欄: Scope | Name | Type | Initial | Comment
      - 4 欄: Scope | Name | Type | Comment （沒有 Initial）
      - 3 欄: Scope | Name | Type （沒有 Initial 也沒有 Comment — Method 常見）
    接續列省略 Scope，欄數會少 1。

    支援兩種 markdown 表格格式：
      A) `InOut:\\n:    | a | b | ... |`           （WebFetch 風格，有定義列表縮排與外圍 |）
      B) `InOut:\\n     Scope | Name | ...\\n---|---|...`（html2text 風格，無外圍 |，無 ":")

    Scope 認得：Inout / Input / Output / Return。Return 會被放到 result["return"] 而非 inout/input/output。
    """
    result = {"inout": [], "input": [], "output": [], "return": []}

    # 先嘗試格式 A：InOut: 後接 `:    |...`
    m = re.search(r"InOut:\s*\n:\s*(\|.*?)(?=\n\n[^|]|\Z)", content, re.DOTALL)
    table = m.group(1) if m else None

    # 退而求其次：格式 B — InOut: 後直接接表格（無 `:` 也無外圍 |）
    # 抓到檔尾（或下一個 `# ` 標題），因為表格 cell 內可能含空行 + 項目符號
    if not table:
        m = re.search(
            r"InOut:\s*\n(.*?)(?=\n#\s|\Z)",
            content,
            re.DOTALL,
        )
        if not m:
            return result
        table = m.group(1)

    # 取得所有非空、非分隔的列
    rows = []
    for raw_line in table.split("\n"):
        line = raw_line.strip()
        if not line:
            continue
        # 容許 row 帶或不帶外圍 |；只有兩端都是 | 時才視為外圍包裹（wrapped 表格），
        # 否則（borderless 表格）保留結尾的 | 因為它是「最後一個空欄」的分隔符。
        if line.startswith("|") and line.endswith("|"):
            line = line[1:-1]
        # 若整列都是 ---|---|... 分隔列，跳過
        if re.fullmatch(r"[\s\-|]+", line):
            continue
        if "|" not in line:
            continue
        cells = [c.strip() for c in line.split("|")]
        if all(c == "---" or c == "" or set(c) == {"-"} for c in cells):
            continue
        rows.append(cells)
    
    if not rows:
        return result
    
    # 第一列是表頭，決定欄數格式
    header = [c.lower() for c in rows[0]]
    if header[:5] == ["scope", "name", "type", "initial", "comment"]:
        full_cols = 5
        has_initial = True
        has_comment = True
    elif header[:4] == ["scope", "name", "type", "comment"]:
        full_cols = 4
        has_initial = False
        has_comment = True
    elif header[:3] == ["scope", "name", "type"]:
        full_cols = 3
        has_initial = False
        has_comment = False
    else:
        # 未知表頭，仍嘗試以 5 欄推斷
        full_cols = 5
        has_initial = True
        has_comment = True

    short_cols = full_cols - 1  # 續列無 scope

    def clean(s: str) -> str:
        s = s.strip()
        s = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", s)  # markdown 連結
        s = s.replace("`", "")
        s = s.replace("\\_", "_")
        return s.strip()

    SCOPE_MAP = {
        "inout": "inout",
        "input": "input",
        "output": "output",
        "return": "return",
        "inout const": "inout",  # CODESYS 對 VAR_IN_OUT CONSTANT 的標示
    }

    current_scope = None
    for cells in rows[1:]:  # 跳過表頭
        n = len(cells)
        init_raw = ""
        comment_raw = ""

        if n == full_cols:
            # 含 Scope 的完整列
            scope_raw = cells[0]
            if full_cols == 5:
                _, name_raw, type_raw, init_raw, comment_raw = cells[:5]
            elif full_cols == 4:
                _, name_raw, type_raw, comment_raw = cells[:4]
            else:  # 3
                _, name_raw, type_raw = cells[:3]
            sl = scope_raw.lower().strip()
            if sl in SCOPE_MAP:
                current_scope = SCOPE_MAP[sl]
            else:
                # 不是 scope label — 也許是省略 Scope 的延續列正好同欄數，跳過
                continue
        elif n == short_cols:
            # 續列：省略 Scope
            if full_cols == 5:
                name_raw, type_raw, init_raw, comment_raw = cells[:4]
            elif full_cols == 4:
                name_raw, type_raw, comment_raw = cells[:3]
            else:  # 3 col, short = 2
                name_raw, type_raw = cells[:2]
        else:
            continue

        if not current_scope:
            continue

        entry = {
            "name": clean(name_raw),
            "type": clean(type_raw),
            "initial": clean(init_raw),
            "comment": clean(comment_raw),
        }
        if entry["name"]:
            result[current_scope].append(entry)
    
    return result


def parse_description(content: str) -> str:
    """從 `# Name (...)` 標題後抽出簡短描述（第一段非表格、非範例的文字）。"""
    # 跳過第一行標題
    lines = content.split("\n", 1)
    if len(lines) < 2:
        return ""
    body = lines[1].strip()
    
    # 找到第一段：通常是 FUNCTION_BLOCK XXX\n\n描述...
    # 或 STRUCT XXX
    # 我們要的是「描述」段落
    paragraphs = re.split(r"\n\s*\n", body)
    
    desc_parts = []
    for p in paragraphs:
        p = p.strip()
        if not p:
            continue
        # 跳過：FUNCTION_BLOCK / STRUCT / TYPE 宣告
        if re.match(r"^(FUNCTION_BLOCK|FUNCTION|STRUCT|TYPE|INTERFACE|VAR_GLOBAL)\s+\S+\s*$", p):
            continue
        # 跳過表格
        if p.startswith("|"):
            break
        # 跳過 Example: / Note: / InOut: 起始的段落
        if re.match(r"^(Example|Note|InOut|Inputs|Outputs)[:\b]", p):
            break
        # 跳過 ![image] 圖片
        if p.startswith("!["):
            continue
        # 跳過 markdown 段落標題（# ...），但不要因此停止；繼續往下找說明文字
        if p.lstrip().startswith("#"):
            continue
        # 把第一段非標籤文字當作描述
        desc_parts.append(p)
        if len(" ".join(desc_parts)) > 200:
            break
    
    desc = "\n\n".join(desc_parts)
    # 清理 markdown 連結 → 純文字
    desc = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", desc)
    desc = desc.replace("\\_", "_")
    return desc.strip()


def parse_definition(markdown_full: str, name: str, type_code: str) -> dict:
    """主要 API：取整頁 markdown，回傳結構化定義。"""
    content = extract_content(markdown_full, name)
    if not content:
        return {
            "name": name,
            "type_code": type_code,
            "extracted": False,
            "error": "Could not locate content section",
        }
    
    result = {
        "name": name,
        "type_code": type_code,
        "extracted": True,
        "description": parse_description(content),
        "raw_content": content,
    }
    
    # 對 FB/Function/Interface/Method/Property/Action 解析 I/O
    if type_code in ("FB", "FN", "IF", "MT", "PR", "AC"):
        io = parse_io_table(content)
        result["inout"] = io["inout"]
        result["input"] = io["input"]
        result["output"] = io["output"]
        result["return"] = io.get("return", [])

    return result


if __name__ == "__main__":
    # 測試 1: MC_MoveAbsolute (5-column with Initial)
    print("=" * 50)
    print("Test 1: MC_MoveAbsolute (5-col table)")
    print("=" * 50)
    sample = Path("/home/claude/sm3/raw_sample.md").read_text()
    result = parse_definition(sample, "MC_MoveAbsolute", "FB")
    print(f"Description: {result['description'][:80]}...")
    print(f"InOut: {len(result['inout'])}, Input: {len(result['input'])}, Output: {len(result['output'])}")
    for x in result["input"][:3]:
        print(f"  in  {x['name']}: {x['type']}")
    
    print()
    # 測試 2: MC_Power (4-column without Initial)
    print("=" * 50)
    print("Test 2: MC_Power (4-col table)")
    print("=" * 50)
    sample2 = Path("/home/claude/sm3/raw_mc_power.md").read_text()
    result2 = parse_definition(sample2, "MC_Power", "FB")
    print(f"Description: {result2['description'][:80]}...")
    print(f"InOut: {len(result2['inout'])}, Input: {len(result2['input'])}, Output: {len(result2['output'])}")
    for x in result2["input"]:
        print(f"  in  {x['name']}: {x['type']} — {x['comment'][:50]}")
    for x in result2["output"]:
        print(f"  out {x['name']}: {x['type']} — {x['comment'][:50]}")
