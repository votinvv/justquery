# JustQuery icons — авторский набор v1 (29 глифов)

Единый стиль: сетка **24×24**, штрих **1.8**, концы и соединения скруглённые
(`stroke-linecap/linejoin: round`), цвет — **currentColor** (наследуется от текста темы).
Заливка только у двух «командных» глифов — `run` и `stop`: глаз мгновенно отличает
действия от предметов. Скругления штрихов рифмуются с радиусами UI (7/10).

## Карта имён → назначение → кодпойнт (PUA)

| Файл | Назначение | Codepoint |
|---|---|---|
| new-query | тулбар: новый запрос | U+E900 |
| open | тулбар: открыть файл | U+E901 |
| save | тулбар: сохранить | U+E902 |
| database | тулбар/деревья: база, объект-подключение | U+E903 |
| schema | дерево метаданных: схема | U+E904 |
| table | дерево: таблица | U+E905 |
| view | дерево: представление | U+E906 |
| sequence | дерево: последовательность | U+E907 |
| function | дерево: функция | U+E908 |
| trigger | дерево: триггер | U+E909 |
| run | сабтулбар редактора: выполнить (filled) | U+E90A |
| stop | сабтулбар: остановить fetch (filled) | U+E90B |
| commit | сабтулбар: commit | U+E90C |
| rollback | сабтулбар: rollback | U+E90D |
| format | сабтулбар: форматировать SQL | U+E90E |
| find | поиск / find bar | U+E90F |
| panel-left | тулбар: переключить Connection Manager | U+E910 |
| panel-tree | тулбар: переключить Metadata Manager | U+E911 |
| close | крестики: вкладки, доки, панель результатов, модалки | U+E912 |
| chevron-right | дизклоужер дерева (закрыт), скролл вкладок → | U+E913 |
| chevron-down | дизклоужер дерева (открыт), комбобоксы | U+E914 |
| chevrons-up | панель результатов: свернуть/развернуть вверх | U+E915 |
| chevrons-down | панель результатов: вернуть вниз | U+E916 |
| plus | доки: добавить подключение | U+E917 |
| trash | доки: удалить подключение | U+E918 |
| refresh | metadata dock: rescan; статус scan | U+E919 |
| plug | тулбар: тумблер connect/disconnect | U+E91A |
| plug-off | вариант тумблера (если состояние решим иконкой) | U+E91B |
| key | запасной (если захотим «ключ» для коннекта) | U+E91C |

## Правила акцентирования (из дизайн-системы)

- Иконки тулбара/панелей: `text_dim`; hover → `text`; disabled → `disabled`.
- **Run — единственная цветная иконка тулбара**: `accent_hi`, когда запуск доступен.
- Тумблер подключения: глиф `plug` всегда; цвет = состояние (`text_dim` отключён,
  `ok` подключён); tooltip «Connect…» / «Disconnect».
- Панельные действия (chevrons-up/down, close) над гридом: `text_dim` → `text` на hover,
  никогда не акцент.

## Сборка в шрифт (Windows/линукс, Node.js)

Иконочный шрифт требует ЗАЛИВОК, а наши SVG — штриховые. Поэтому два шага:
сначала разворачиваем штрихи в контуры, затем пакуем в TTF.

```bash
npm i -g oslllo-svg-fixer fantasticon
# 1) штрихи -> заливки (создаст icons_fixed/)
oslllo-svg-fixer -s icons -d icons_fixed
# 2) упаковка в шрифт с фиксированными кодпойнтами
fantasticon icons_fixed -o build --font-types ttf \
  --name justquery-icons --codepoints codepoints.json
```

`codepoints.json` (имена без расширения → десятичные значения U+E900…U+E91C):
сгенерируй из таблицы выше: `{"new-query":59648,"open":59649,...}` — по порядку, E900=59648.

## Интеграция в egui

1. Положи `build/justquery-icons.ttf` в `assets/`.
2. В `theme.rs::setup_fonts` добавь шрифт КАК lucide (отдельный `font_data` +
   push в Proportional и Monospace фолбэки). Lucide на переходный период не удаляем.
3. Создай `src/icons.rs`: `pub const RUN: &str = "\u{E90A}";` и т.д. по карте.
4. Меняй call sites с lucide-глифов на `icons::*` — по фазе 8 todo.md. После полного
   перехода lucide можно удалить вместе с его include_bytes.

Проверка качества после сборки: глифы на 15–16px должны держать штрих ~1.5px без каши —
если где-то мылится, виноват шаг stroke-to-fill; перегенерируй с `--traced false` у
svg-fixer или проверь самопересечения в конкретном SVG.
