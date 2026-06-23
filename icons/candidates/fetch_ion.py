# -*- coding: utf-8 -*-
"""Качает Ionicons outline под наши глифы в icons/candidates/_ion/.
Имена файлов = ключи codepoints.json (+ новый 'check'). Нормализует SVG:
убирает class="ionicon" и суффикс px у stroke-width."""
import os, re, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
DST = os.path.join(HERE, "_ion")
os.makedirs(DST, exist_ok=True)
CDN = "https://cdn.jsdelivr.net/npm/ionicons/dist/svg/{}.svg"

# наш_глиф(имя файла) -> Ionicons outline
MAP = {
 "new-query":"document-outline", "open":"folder-open-outline", "save":"save-outline",
 "database":"server-outline", "schema":"document-text-outline", "table":"grid-outline",
 "view":"eye-outline", "sequence":"reorder-four-outline", "function":"code-slash-outline",
 "trigger":"flash-outline", "run":"play", "stop":"flash",
 "commit":"git-commit-outline", "rollback":"arrow-undo-outline", "format":"code-outline",
 "find":"search-outline", "panel-left":"server-outline", "panel-tree":"library-outline",
 "close":"close-outline", "chevron-right":"chevron-forward-outline",
 "chevron-down":"chevron-down-outline",
 # chevrons-up / chevrons-down — рукотворные двойные шевроны (как save-as), не из Ionicons
 "plus":"add-outline", "trash":"trash-outline",
 "refresh":"refresh-outline",
 # plug / plug-off — рукотворная вилка (Ionicons её не имеет), форма как в старом наборе
 "key":"key-outline", "check":"checkmark-outline",
}

def norm(svg):
    svg = svg.replace(' class="ionicon"', '')
    svg = re.sub(r'stroke-width="([\d.]+)px"', r'stroke-width="\1"', svg)
    return svg.strip() + "\n"

for stem, ion in MAP.items():
    with urllib.request.urlopen(CDN.format(ion), timeout=30) as r:
        svg = r.read().decode("utf-8")
    with open(os.path.join(DST, f"{stem}.svg"), "w", encoding="utf-8") as f:
        f.write(norm(svg))
    print(f"  {stem:14s} <- {ion}")
print(f"OK -> {DST}  ({len(MAP)} files)")
