# JustQuery icons — набор на основе Ionicons (31 глиф)

С 2026-06 набор переведён на **Ionicons** (MIT) — тонкие линии. Исходная сетка
**512×512**, штрих **~32–48**, концы/соединения скруглённые, цвет — **currentColor**.
Командные `run`/`stop` — **залитые** (Ionicons `play` / `flash`), цвет несёт смысл
(Execute зелёный, Stop красный); остальные — контурные (`*-outline`). Смысловые
замены, где у Ionicons нет аналога: `database / Connection mgr → server`,
`Metadata mgr → library`, `schema / XML-model → document-text`, `table → grid`,
`sequence → reorder-four`, `function → code-slash`, `format → code <>`,
`connect → enter`, `disconnect → exit`, `trigger → flash`. Нарисованы вручную
(не из Ionicons): `save-as` (floppy + «+» в углу), `chevrons-up`/`chevrons-down`
(двойные шевроны формы одиночного `chevron-down`). Полностью рисованный в коде
остаётся только `paint_cross` (закрывающий крестик — в шрифте кодпойнта нет).

Глифы скомпилированы в `assets/justquery-icons.ttf`; исходные SVG лежат в `icons/*.svg`,
карта «имя → кодпойнт» — в `icons/codepoints.json`.

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
| check | Inspect/Validate | U+E91D |
| save-as | Save As (floppy + «+») | U+E91E |

## Правила акцентирования (из дизайн-системы)

- Иконки тулбара/панелей: `text_dim`; hover → `text`; disabled → `disabled`.
- **Командные иконки несут цвет**: Execute — `ok` (зелёный), когда запуск доступен;
  Stop — `danger` (красный), пока что-то выполняется.
- Тумблер подключения: глиф `plug` всегда; цвет = состояние (`text_dim` отключён,
  `ok` подключён); tooltip «Connect…» / «Disconnect».
- Панельные действия (chevrons-up/down, close) над гридом: `text_dim` → `text` на hover,
  никогда не акцент.

## Интеграция в egui

1. `assets/justquery-icons.ttf` грузится в `theme.rs::setup_fonts` как fallback `jq-icons`
   (push в хвост Proportional и Monospace), чтобы PUA-кодпойнты рендерились в любом
   текстовом ране.
2. Константы кодпойнтов — в `src/icons.rs` (`pub const RUN: &str = "\u{E90A}";` и т.д.,
   по карте выше). Единственный рисованный в коде глиф — `icons::paint_cross`.
