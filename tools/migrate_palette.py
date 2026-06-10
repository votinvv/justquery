"""One-shot: rewrite deprecated theme colour consts to theme::p().<field> using
exact byte spans from rustc's JSON diagnostics (no false hits inside strings).
Spans on `use` lines are skipped (imports get cleaned by hand afterwards)."""
import json
import subprocess
import sys
from collections import defaultdict

FIELD = {
    "IVORY": "ivory", "PANEL": "panel", "PANEL2": "panel2", "GRID_HEADER": "grid_header",
    "DATA_BG": "data_bg", "ROWALT": "row_alt", "BORDER": "border",
    "BORDER_STRONG": "border_strong", "MENU_BORDER": "menu_border", "DIVIDER": "divider",
    "TEXT": "text", "TEXTDIM": "text_dim", "DISABLED": "disabled", "ACCENT": "accent",
    "HOVER": "hover", "SELECT": "select", "ACC_BG": "acc_bg", "ACC_BG2": "acc_bg2",
    "OK": "ok", "WARN": "warn", "DANGER": "danger",
    "SCROLL_DORMANT": "scroll_dormant", "SCROLL_HOT": "scroll_hot",
    "SCROLL_PRESSED": "scroll_pressed", "GRIP_HOT": "grip_hot", "FIND_HL": "find_hl",
    "GUTTER": "gutter", "ACTIVE_LINE": "active_line", "EDITOR_SEL": "editor_sel",
    "SYN_KW": "syn_kw", "SYN_STR": "syn_str", "SYN_COM": "syn_com", "SYN_NUM": "syn_num",
    "SYN_FN": "syn_fn", "ACCENT_PRESS": "accent_press", "SELECT_BAR": "accent",
}

subprocess.run(["cargo", "clean", "-p", "justquery", "--release"], check=True)
out = subprocess.run(
    ["cargo", "build", "--release", "--message-format=json"],
    capture_output=True, text=True, encoding="utf-8",
).stdout

# (file) -> list of (byte_start, byte_end, replacement)
edits = defaultdict(list)
skipped_use = 0
for line in out.splitlines():
    try:
        msg = json.loads(line)
    except json.JSONDecodeError:
        continue
    if msg.get("reason") != "compiler-message":
        continue
    m = msg["message"]
    if (m.get("code") or {}).get("code") != "deprecated":
        continue
    span = next((s for s in m["spans"] if s["is_primary"]), None)
    if span is None:
        continue
    src_line = span["text"][0]["text"] if span["text"] else ""
    if src_line.lstrip().startswith("use "):
        skipped_use += 1
        continue
    # span text is the path as written: e.g. `TEXT`, `crate::PANEL`, `theme::OK`
    name = src_line[span["column_start"] - 1 : span["column_end"] - 1]
    bare = name.rsplit("::", 1)[-1]
    if bare not in FIELD:
        print(f"??  unknown const {name!r} at {span['file_name']}:{span['line_start']}")
        continue
    edits[span["file_name"]].append(
        (span["byte_start"], span["byte_end"], f"p().{FIELD[bare]}".encode())
    )

total = 0
for path, spans in edits.items():
    data = open(path, "rb").read()
    for start, end, repl in sorted(spans, key=lambda e: -e[0]):
        data = data[:start] + repl + data[end:]
        total += 1
    open(path, "wb").write(data)
    print(f"{path}: {len(spans)} replacements")
print(f"done: {total} replacements, {skipped_use} use-line spans left for manual cleanup")
