# Тотальный рефакторинг-аудит JustQuery — итоговый отчёт

> Сгенерирован 2026-06-15 многоагентным аудитом (15 единиц: 12 подсистем + зависимости/кросс-дубли/concurrency,
> каждая находка прошла адверсариальную верификацию). Подтверждено/скорректировано: **103**, отклонено: **7**.

## Сводка (приоритетные действия)

- **Чинить квадратичный поиск закрывающего dollar-тега в `sqlfmt::tokenize`** (`src/sqlfmt.rs:186-193`): `cs[j..].iter().collect::<String>()` на каждом шаге → O(n²) и шквал аллокаций на больших `$$…$$`. Рядом в этом же файле (1524-1525) уже есть корректный срез-сравнение — применить тот же приём. Единственная находка severity **high**, движок хоть и запаркован, дефект объективен. [S]
- **Отменять фоновые процессы при закрытии вкладки** (`src/main.rs:1341-1354`): `close_tab` делает голый `remove(i)` без `cancel.store(true)` — воркеры Format/Validate/Search и долгий SQL-воркер продолжают жечь CPU и держать mmap-снимок/серверную сессию. Выставлять `cancel` (и слать `CancelRequest` для SQL) перед удалением. [S]
- **Убрать двойной клон строки в горячем пути редактора** (`src/codeeditor.rs:729,796,921`): `get_line()` всегда `.to_owned()`, строка клонируется дважды на каждую видимую строку каждый кадр. Передавать уже прочитанный `text` в `hl_line` параметром. [M]
- **Кэшировать раскладку дерева метаданных** (`src/metadata.rs:485-549`): `metadata_tree_body` без виртуализации каждый кадр пересобирает группировку и строит `format!`-ключи дважды на папку/объект — сотни-тысячи аллокаций String/кадр. Кэшировать `folders`+плоский `visible` по `meta_view_gen`/составу открытых папок; ключи строить один раз. [M]
- **Ограничить рост памяти piece table и undo-журнала** (`src/doc/piece_table.rs:136-184`, `src/doc/mod.rs:89`): add-буфер только растёт, delete не освобождает байты, `undo: Vec<Vec<EditItem>>` без лимита. Компактизация add при swap_origin/save + лимит глубины журнала. [M]
- **Лексеры SQL без материализации строки в Vec** (`src/highlight.rs:65,170`): `char_indices().collect()`/`chars().collect()` на каждую видимую строку при перелэйауте. Переписать на проход по `char_indices()` напрямую (как `xmlhl.rs` по `&[u8]`); `is_keyword` (44-57) — без lowercase-аллокации (бинарный поиск/`OnceLock<HashSet>`). [M]
- **Защита от пересборки автодополнения вхолостую** (`src/complete.rs:355,393-405`): `ac_refilter` каждый кадр клонирует весь отфильтрованный `Vec<AcItem>` при открытом popup, даже когда префикс не менялся. Хранить `last_prefix` и фильтровать только при изменении. [S]
- **Подключить уже написанную `doc::cleanup_temp_dir`** (`src/doc/mod.rs:840-851`): функция-подметальщик temp-каталога существует, но нигде не вызывается. Звать однократно при старте рядом с `update::startup_cleanup()` — закрывает осиротевшие temp-файлы форматирования. [S]
- **Унифицировать `vbar`/`hbar` и три семейства кнопок** (`src/vscroll.rs:20-85`, `src/widgets.rs:563-672`): зеркальные по осям / идентичные по скелету реализации — параметризовать осью/набором цветов. Снижает риск рассинхрона при правках. [M]
- **Гасить или подключать мёртвые пункты меню** (`src/menubar.rs`): ~13 кликабельных пунктов (Replace…, Go to Line…, Commit, Execute, Stop, Export Result…, Preferences…, Next/Previous Tab, Close All Tabs, Documentation…) ничего не делают по клику, но рекламируют горячие клавиши — вводит в заблуждение. [S]

---

## 1. Актуальность библиотек

Серьёзных проблем не найдено. Дерево зависимостей свежее и здоровое.

- **Все прямые зависимости актуальны** — `Cargo.toml`, `Cargo.lock`. eframe/egui 0.34.3, postgres 0.19.13, quick-xml 0.40.1, wgpu/naga 29.0.3, winit 0.30.13, ureq 3.3.0 и пр. соответствуют последним стабильным. Действий не требуется; самой дорогой будущей миграцией станет egui/wgpu 0.34→0.35 (отдельным коммитом, с чтением CHANGELOG и UI-смоуком на DX12 + WARP). Неточность исходного аудита: `winresource` в lock резолвится на 0.1.31, а не 0.1.26 — на вывод не влияет. [severity: low | усилия: —]
- **Дубли версий в `Cargo.lock` платформенно-обусловлены** — `quick-xml 0.39.4` приходит ИСКЛЮЧИТЕЛЬНО через `wayland-scanner` (Linux/Wayland-ветка winit, не компилируется под Windows и в exe не попадает); `bitflags 1.3.2 vs 2.x`, `calloop 0.13 vs 0.14` — норма большого GUI-графа. Прямой зависимостью не схлопываются. Не находка. [severity: low | усилия: —]

---

## 2. Мусорный код

- **Мёртвые пункты меню — заглушки без действия** — `src/menubar.rs:180,182,192,193,196,197,199,225,228,229,240,243,244`. `item(ui,…)` при клике лишь делает `ui.close()`, тела действия нет, но пункты рекламируют Ctrl+H/Ctrl+G/F8/Esc/Ctrl+Tab. Export Result… и Preferences… не реализованы нигде; Stop/Execute/Next-Prev Tab/Close All живут только инлайном/кнопками. Привязать к обработчикам либо пометить `disabled` через `item_en(…, false)`. [severity: medium | усилия: M]
- **`fmt_elapsed` мёртв, комментарий обманывает** — `src/main.rs:338-351`. Единственное вхождение — определение; `finish_proc` форматирует длительность вручную (`format!("{secs:.1}")`, 2866-2867), функция (~14 строк) недостижима. Удалить либо реально вызвать в `finish_proc`. [severity: low | усилия: S]
- **Поле `EditorOut::changed_from` не читается** — `src/codeeditor.rs:518-523`. Struct-уровневый `#[allow(dead_code)]` глушит детект; все поля кроме `changed_from` читаются в `main.rs:2586-2598`. Заменить struct-allow на точечную пометку реально мёртвого поля (инвалидация идёт через `doc.take_change_start`), чтобы компилятор снова ловил новый мусор. [severity: low | усилия: S]
- **`menu_end = 220.0` всегда перезаписывается** — `src/menubar.rs:50`. Значение никогда не читается (переприсваивается на 67). Объявить `let menu_end;` без магической константы. [severity: low | усилия: S]
- **Широкие файловые `#![allow(dead_code)]` маскируют мусор** — `src/widgets.rs:5`, `src/dialog.rs:6`. В widgets под allow реально мёртв `qbtn_dim` (плаг-тоггл переехал на `qbtn`) — удалить; `destructive_button`/`secondary_button` (не `_w`) — осознанные API-обёртки, оставить, но пометить точечно. В dialog почти всё используется (`now_hms`, `message_box`, `open_file`/`save_file` и т.д.) — сузить allow до FFI-констант `OFN_*` и ABI-полей `OpenFileNameW`. [severity: low | усилия: S]
- **`ProcKind::finished_word` не используется** — `src/proc.rs:34-37`. Парный `stopped_word` используется в `finish_proc`, `finished_word` — нет (итоги Done собираются хардкодом). Удалить либо задействовать ради симметрии. [severity: low | усилия: S]
- **Неиспользуемый derive `PartialEq` у `AcKind`** — `src/complete.rs:14`. Сравнений нет, читается только через `match`. Убрать `PartialEq` (оставить `Clone, Copy`). [severity: low | усилия: S]
- **Рудиментарные параметры сигнатур** — `src/validate.rs:485` (`_pos` в `on_text`), `src/format.rs:226-231` (`_reader: &Reader<R>` в `write_start_tag`, параметр `R` держится только ради него). Удалить из сигнатур и вызовов. [severity: low | усилия: S]

> Примечание: запаркованный путь XML Run (`ProcKind::Run`, `ProcMsg::Tables`, `start_xml_run`, `shred.rs`) под `#[allow(dead_code)]` — **намеренная парковка**, аналог sqlfmt; не мусор (см. раздел «Ложные тревоги»).

---

## 3. Дублирование кода

- **`vbar`/`hbar` — зеркальная копия по осям** — `src/vscroll.rs:20-51, 54-85`. ~30 строк f64-арифметики (clamp 0.05..1.0, len.max(24.0), маппинг указателя, выбор цвета, `CornerRadius::same(4)`) повторены дословно с заменой y↔x. Свести к `fn bar(ui, track, id, offset, content, axis)`; vbar/hbar — тонкие обёртки. [severity: medium | усилия: M]
- **Сканер строк/комментариев/dollar-блоков в `split_statements` vs `split_statements_lines`** — `src/connections.rs:402-490, 501-596`. Посимвольно повторённый сканер квотинга (`--`, `/* */`, `'…''…'`, `$tag$`) — критичный инвариант, дублированный в двух местах. `split_statements` уже `#[allow(dead_code)]` (для тестов) — сделать его обёрткой над линейной версией: один источник правды. [severity: medium | усилия: S]
- **Три семейства кнопок почти идентичны** — `src/widgets.rs:563-672`. `primary_button_w`/`destructive_button_w`/`secondary_button_w` повторяют скелет (allocate→disabled→fill-по-состоянию→текст), отличаясь тройкой цветов и шрифтом. Свести к `filled_button(…, fill_rest, fill_hover, fill_press, fg, font)` + три обёртки. [severity: medium | усилия: M]
- **Прологи `start_xml_*`/`start_search` и сборка `RunningProc`** — `src/main.rs:2640-2669, 2672-2694, 2698-2725, 2729-2749`. Идентичный пролог (`cancel`, `channel`, `search_hl.clear()`) + литерал `RunningProc{… capped:false}` × 4. Добавить `RunningProc::new(kind, rx, cancel, schema)` (в proc.rs уже есть `impl RunningProc`) и/или `Tab::begin_proc`. [severity: medium | усилия: S]
- **Троттлинг прогресса воркеров** — `src/format.rs:206-209`, `src/validate.rs:273-276`, `src/search.rs:136-139`. `if p-last>=1.0 { last=p; tx.send(Progress(p.min(cap))) }` × 3 (кап 99/99/100). Вынести `ProgressThrottle{ last }.maybe_send(tx, pct, cap)` в proc.rs. [severity: medium | усилия: S]
- **Построение `Finding{code/source:"XSD"}` ~10 раз** — `src/validate.rs:335-654`. Литерал `code/source="XSD"` + структура Finding повторены в on_start/on_text/close_top/check_attrs, отличаясь только message. Хелперы `xsd_error(line, msg)`/`xml_error(…)`. [severity: medium | усилия: S]
- **Дублирование `stored`/`can_apply` между телом scan_tab и scan_toolbar** — `src/meta_manager_modal.rs:88-92, 388-390, 466-475`. Любое новое поле настроек коллектора требует синхронной правки в обоих `can_apply`. Хелперы `stored_meta_settings()`/`meta_can_apply()`. [severity: medium | усилия: S]
- **Дублированный парсинг dollar-quoted: `tokenize` vs `format_dollar_blocks_in`** — `src/sqlfmt.rs:180-205, 1517-1537`. Две копии цикла поиска закрытия разошлись по качеству (медленная сборка String против среза). Общий `fn dollar_close(cs, open_end, tag) -> Option<usize>` — заодно чинит perf-находку из раздела 7. [severity: medium | усилия: S]
- **«Серебристый лист» (data sheet) дублируется ~4 раза** — `src/about.rs:177-186`, `src/metadata.rs:567-580`, `src/connections_ui.rs:1101-1111`, `src/meta_manager_modal.rs:95-104`. CentralPanel→`island_shadow_under`→`island_box`→`style_scrollbar`→`ScrollArea` (часть ещё + `style_modal_widgets`). Хелпер `widgets::data_sheet(ui, |ui|{…})`. [severity: medium | усилия: M]
- **Каркас бокового док-списка дублируется** — `src/connections_ui.rs:421-579`, `src/metadata.rs:413-439`. Island Frame+clip+scrollbar+`crisp_border` поверх — совпадают даже комментарии. Хелпер `widgets::dock_list_island(ui, |ui|{…})`. [severity: medium | усилия: M]
- **Цикл `reset_session` по вкладкам повторён трижды; `kill_all`≈`disconnect_now`** — `src/connections_ui.rs:69-71, 145-147, 171-175`. Завести `reset_all_sessions(&mut self)`, звать из трёх мест; `kill_all` — тонкая обёртка. [severity: medium | усилия: S]
- **UTF-16 wide-конверсия для WinAPI** — `src/dialog.rs:135-137, 92`, `src/update.rs:310-313`. `dialog::wide`, локальное замыкание `wide` в `message_box` (можно убрать — рядом готовая функция) и `update::to_wide` дают идентичный результат. Общий `to_wide`/`from_wide` в win-утилите. [severity: medium | усилия: S]
- **Снятие обёрток `db error: `/`ERROR: ` с серверной ошибки** — `src/connections.rs:607-610, 657-659`. `unwrap_pg_error(chain)->&str`. (Замечание: `prepare_error` запаркован под `#[allow(dead_code)]`, так что один экземпляр сейчас мёртв — выгода ограничена до возврата фичи.) [severity: low | усилия: S]
- **Идентичный блок навигационных клавиш в `editor_input`/`editor_nav_input`** — `src/codeeditor.rs:957-973, 1084-1096` (+ Ctrl+A на 943/995, `Event::Copy` на 949-956/1026-1033). Вынести `apply_nav_key(ed, doc, key, mods, …)`. [severity: low | усилия: M]
- **Предикат `can_save` вкладки соединения скопирован дословно** — `src/connections_ui.rs:1090-1095, 1189-1194` (+ согласовать с `save_conn_tab:824-831`). `fn conn_tab_can_save(&self, idx)->bool`. [severity: low | усилия: S]
- **`next_new_conn_name` vs `free_variant` — почти один счётчик свободного имени** — `src/connections_ui.rs:645-655, 658-673`. Это duplication (функции живые, не dead_code). `first_free<F>(start, exclude_id, fmt)`. [severity: low | усилия: S]
- **Take/restore `ctx.event` в правилах 5.6/5.16/5.17/5.27** — `src/rules/mod.rs:269-382`. `with_event(ctx, |ev, ctx|{…})` убирает класс ошибок «забыли вернуть event». [severity: low | усилия: S]
- **`required_if`/`forbidden_if` различаются лишь предикатом** — `src/rules/mod.rs:602-645`. Общий генератор с замыканием-предикатом (унифицирует и вычисление `line`). [severity: low | усилия: S]
- **Ручной клон вместо `derive(Clone)`** — `src/xsd/loader.rs:379-390` (`clone_facets`, 8 полей), `271-275` (`AttrDecl`, 3 поля). Добавить `#[derive(Clone)]` к `Facets`/`AttrDecl`, заменить на `.iter().cloned().collect()`. Риск молчаливой потери фасета при будущем расширении. [severity: low | усилия: S]
- **Прочие мелкие дубли (low, усилия S):** reorder-индекс `remove+insert` ×3 — `src/grid.rs:46-49, 230-235`, `src/widgets.rs:418-426` → `reorder_index(from,to,len)`; скелет hover-box `qbtn_*`/`qchevron` ×5 — `src/widgets.rs:129-146,155-177,279-288` → `hover_box(ui,rect,active)`; `match buf(src)` — `src/doc/piece_table.rs:77-82,230-235,260-262`; `attach_origin` vs `reattach_same_content` mmap-блок — `src/doc/mod.rs:215-230, 804-820` → `map_origin(path)`; `strip_prefix('v'/'V')` — `src/update.rs:64,93`; `protect`/`unprotect` ~90% копипаста — `src/crypt.rs:45-100`; `now_hms`/`now_datetime` пролог GetLocalTime — `src/dialog.rs:70-85`; `fmt_size` vs ручной «{} KB» — `src/meta_collector.rs:425-431`, `src/about.rs:270-273`; depth +=/-=1 паттерн — `src/sqlfmt.rs` (множество мест); каркас `LineState` key/from_key — `src/highlight.rs:24-42` vs `xmlhl.rs` (намеренная архитектура, инвариант защищён тестом `state_only_matches_full`); стиль dropdown-строк меню — `src/menubar.rs:96-105,205-206`; блок Stop в двух тулбарах — `src/main.rs:2293-2301,2337-2350` (выгода невелика — ветки действий разные).

---

## 4. Упрощение/унификация компонент

- **Классификация действия обновления продублирована** — `src/about.rs:139-166, 327-354`. `about_toolbar` (булевы checking/busy_dl/can_download) и футер (match по `&status`) кодируют одно отображение `UpdateStatus→действие`. Вынести `fn update_action(&self)->UpdateAction`. [severity: low | усилия: S]
- **`reports: Vec<(usize,)>` — однополевой кортеж** — `src/rules/mod.rs:606-622`. Рядом `forbidden_if` использует `Vec<usize>`. Сделать `Vec<usize>`. [severity: low | усилия: S]
- **Двухпроходный взаимозависимый расчёт `need_v`/`need_h`** — `src/codeeditor.rs:634-645`. Хрупко при правках. Вынести в чистую `resolve_bars(content_h, total_w, view, bar)->(need_v,need_h,inner)` с юнит-тестом граничных случаев. [severity: low | усилия: S]
- **`ensure_line_cached`: двойной lookup по HashMap** — `src/doc/mod.rs:327-340`. `contains_key` + `get` по одному ключу даже в hit-ветке. Перестроить под один lookup (insert при промахе, один `get` в конце). [severity: low | усилия: S]
- **Снапшот всего `MetaStore` глубоким клоном под read-lock** — `src/metadata.rs:195-200`. Две полные копии каталога (store + meta_view). Рассмотреть `meta_view: Arc<MetaStore>` со свапом Arc (COW) — убирает вторую копию и удешевляет Refresh. [severity: low | усилия: M]
- **`compile_complex_body`: хрупкий критерий «база не скомпилирована»** — `src/xsd/loader.rs:258-269`. Эвристика по пустоте `particle/attrs` не различает «каркас» и «реально пустой тип» → редкая лишняя перекомпиляция. Явный флаг скомпилированности (`HashSet<usize>`). [severity: low | усилия: S]
- **`flush_inline_as_lines`: двойная проверка пустоты** — `src/format.rs:284-315`. `inline_is_empty` + повторный `trimmed.is_empty()`. ВНИМАНИЕ: не чистый дубль — `inline_is_empty` трактует любой CData как непустой, а финальный trim отбросил бы `<![CDATA[]]>`. Упрощать с учётом CData-семантики, иначе изменится вывод. [severity: low | усилия: S]
- **`find_bar`: магические константы геометрии + ручная лупа** — `src/find.rs:55,82-115`. Слагаемые `row_w` — в именованные `let`. Для лупы нужен НОВЫЙ хелпер квадратной обведённой иконочной кнопки (`chip_button` не подходит — без рамки и квадратной геометрии). [severity: low | усилия: S]
- **Блок `visuals.widgets` меню перестраивается каждый кадр** — `src/menubar.rs:28-48`. ~20 присваиваний из `p()` в immediate-mode пути; значения зависят только от темы. Вынести `menu_label_style(ui)` (читаемость; стоимость присваиваний мала). Это уникальный inline-стиль, НЕ дубль `theme::apply`. [severity: low | усилия: S]
- **Прочие (low, S):** общий `local_time()->SystemTimeW` под `now_hms`/`now_datetime` (`src/dialog.rs:70-85`); `style_dropdown(ui)` для повторяющегося стиля меню/подменю (`src/menubar.rs:96-105,205-206`).

---

## 5. Возможные зависания

Критичных UI-дедлоков и нарушений инварианта лексера не найдено. Реальные пункты — фоновые «зомби»-потоки при закрытии вкладки (UI остаётся отзывчивым).

- **Долгий SQL-запрос при закрытии/дисконнекте не получает `CancelRequest`** — `src/main.rs:786-797 (reset_session), 800-869`, `src/connections_ui.rs:138-150, 171-175`. `reset_session`/`kill_all`/`disconnect_now` лишь сбрасывают `exec_rx`; воркер блокирован в синхронном `run_statement` до ответа сервера. `CancelToken` есть (`cancel_running_query`), но вызывается ТОЛЬКО кнопкой Stop. При массовом закрытии вкладок на медленной БД копятся заблокированные потоки + занятые серверные сессии. Слать `CancelRequest` в `reset_session`/`kill_all` перед сбросом. [severity: low | усилия: S]
- **`download_and_install`: read-цикл без флага отмены** — `src/update.rs:204-212`. Цикл чтения тела без cancel-флага (в отличие от `spawn_shred` с `Arc<AtomicBool>`). Это фоновый поток (UI отзывчив) — вопрос управляемости загрузки, не UI-hang. Добавить `cancel` по образцу `spawn_shred` либо детект отсутствия прогресса. [severity: low | усилия: S]

> Связанная утечка-ресурсов при закрытии вкладки с Format/Validate/Search — см. раздел 6 (та же первопричина: `close_tab` не выставляет `cancel`).

> `depth` в минус при несбалансированных скобках (`src/sqlfmt.rs:520-543`) — **НЕ зависание**: `check_parens` гейтит ввод в `parse_all:1300-1303` раньше. См. «Ложные тревоги».

---

## 6. Возможные утечки памяти

- **add-буфер piece table и undo-журнал растут неограниченно** — `src/doc/piece_table.rs:136-184`, `src/doc/mod.rs:89`. `insert` всегда `add.extend_from_slice`, `delete` правит только pieces (байты остаются живыми), `undo: Vec<Vec<EditItem>>` без лимита (хранит копии old/new байт). Долгая сессия (набор+удаление/многократный формат без swap_origin) → монотонный рост RAM по обоим каналам. Компактизация add при swap_origin/save или по порогу мусора + лимит глубины undo (по числу шагов/суммарным байтам). [severity: medium | усилия: M]
- **Закрытие вкладки с активным процессом не отменяет воркер** — `src/main.rs:1341-1354`, `src/proc.rs:119-130`, `src/format.rs:39-61`, `src/validate.rs:44-65`, `src/search.rs:26-32`. `close_tab` делает только `remove(i)`; воркеры выходят ТОЛЬКО по `cancel.load`, на разрыв `rx` не реагируют. На большом XML: зомби-поток жжёт ядро + удерживает живым `PieceSnapshot` (mmap оригинала, файл нельзя освободить/перезаписать) до естественного конца прогона. Перед `remove(i)`: `if let Some(rp)=self.tabs[i].proc.as_ref(){ rp.cancel.store(true, Relaxed); }`. Аналогично при выходе из приложения. [severity: medium | усилия: S]
- **Осиротевший temp-файл форматирования при закрытии-во-время-Format** — `src/main.rs:1333-1354`, `src/format.rs:39-60`. `request_close_tab` проверяет только `dirty()`, не `proc`; при успехе воркер шлёт `FormatOk{out_path}`, но `tx.send` после `remove` молча провалится → temp-файл осиротеет. Дёшево: выставлять `cancel` в `close_tab` (см. выше); шире — однократно звать уже написанную, но неподключённую `doc::cleanup_temp_dir(max_age_s)` (`src/doc/mod.rs:840-851`) при старте. [severity: low | усилия: S]
- **`TrackingReader` копит копии прочитанных байт до «потребления» `line_at`** — `src/validate.rs:85-107`. `read` складывает `data.to_vec()` в `Tracker.chunks`; `on_text` (Text/CData/GeneralRef) не зовёт `line_at`, поэтому `consumed` не продвигается внутри простого элемента — хвост чанков копится. Ограничено read-ahead буфером (не «весь поток»). Считать `\n` прямо в `read` (memchr) и держать лишь разреженные контрольные точки offset→строки. [severity: low | усилия: M]

> `LineCache` (`src/codeeditor.rs:38-67`) — **НЕ утечка**: двухпоколенный кэш с жёсткой границей ~2×CAP (swap+clear). См. «Ложные тревоги».

---

## 7. Неоптимальные алгоритмы

- **Квадратичный поиск закрывающего dollar-тега** — `src/sqlfmt.rs:186-193`. `cs[j..].iter().collect::<String>().starts_with(&tag)` собирает весь остаток в String на каждом шаге → O(n²) + шквал аллокаций на больших plpgsql-телах. Корректный аналог рядом (1524-1525: `cs[j..j+tl] == tag[..]`). Это единственная **high**-находка; объективный дефект, хоть движок и запаркован. [severity: high | усилия: S]
- **`char_pos` сканирует весь хвост чанка на каждом вызове** — `src/doc/mod.rs:448-454`. `pt.read(byte_base, line_start-byte_base)` + `count_chars` без кэша символьной базы строки. Зовётся КАЖДЫЙ кадр в статус-баре (`main.rs:2589`). У конца большого чанка — аллокация Vec до сотен КБ + линейный скан/кадр. Кэшировать `char_base` начала текущей строки, инвалидируя вместе с `line_cache`. [severity: medium | усилия: M]
- **Дерево метаданных пересобирается каждый кадр** — `src/metadata.rs:485-549`. `metadata_tree_body` без виртуализации: пересборка `folders`, полный проход группировки, `format!`-ключ папки ДВАЖДЫ (507/521), ключ объекта дважды (509/535) — при сотнях видимых объектов сотни-тысячи аллокаций String/кадр. Строить ключи один раз; кэшировать раскладку по `meta_view_gen`/составу открытых папок. [severity: medium | усилия: M]
- **`get_line()` клонирует строку дважды на видимую строку/кадр** — `src/codeeditor.rs:729,796,921`. `get_line` всегда `.to_owned()`; клон на 729 и повторно на 921 внутри `hl_line`. На мегабайтной строке — две полные копии в горячем пути. Передавать прочитанный `text` параметром в `hl_line` (убирает второй клон сразу). [severity: medium | усилия: M]
- **SQL-лексеры материализуют строку в Vec** — `src/highlight.rs:65,170`. `char_indices().collect()` / `chars().collect()` на каждую видимую строку при перелэйауте (парный `xmlhl.rs` работает по `&[u8]` без Vec). Переписать на проход по `char_indices()` напрямую. [severity: medium | усилия: M]
- **`is_keyword`: lowercase-аллокация + линейный скан ~60 элементов на каждое слово** — `src/highlight.rs:44-57`. В построчном колбэке подсветки. `eq_ignore_ascii_case` по отсортированному массиву + бинарный поиск, либо `OnceLock<HashSet>`/phf. (`complete.rs:123-131 is_kw` — тот же паттерн, но на F6, не на кадр — приоритет ниже.) [severity: medium | усилия: S]
- **`ac_refilter` клонирует весь отфильтрованный список каждый кадр при открытом popup** — `src/complete.rs:355,393-405`. Фильтрует `ac.all` и `.cloned()` собирает `Vec<AcItem>` (по две String) + дважды `to_lowercase` на элемент, даже когда prefix не менялся. Хранить `last_prefix`, фильтровать только при изменении; сравнивать без аллокации. [severity: medium | усилия: S]
- **Galley строки каретки пересчитывается до 3 раз/кадр** — `src/codeeditor.rs:624,796,806`. `hl_line` для строки каретки на 624/796/806. Galley берётся из кэша (не растеризуется), но каждый вызов строит ключ копированием всего текста (922-925) → аллокация ключа на каждую видимую строку. Построить ключ без копии всей строки (хэш `(state,text)`/Cow); вычислять live_galley один раз после мыши. [severity: low (ближе к medium) | усилия: M]
- **`pos_to_byte` клонирует всю строку ради байт-смещения колонки** — `src/doc/mod.rs:430-444`. `get_line().to_owned()` при `col>0`, хотя нужно лишь `char_indices().nth(col)`. На правках/копировании (не каждый кадр), но на длинных строках клон+UTF-8-проверка дороги. Читать только префикс до col / брать `&str` из `ensure_line_cached` без `to_owned`. [severity: medium | усилия: S]
- **O(n·m) подсчёт номера строки пересканированием от начала** — `src/sqlfmt.rs:719,746,1406`. `1 + sql.chars().take(pos).filter(|c| *c=='\n').count()` на каждый хит в pub-функциях валидации (`subquery_lines`/`select_star_lines`/`unqualified_lines`, зовутся из `validate` — путь ЖИВОЙ). Один раз построить отсортированный вектор позиций `\n`, считать строку через `partition_point` (O(log m)). [severity: medium | усилия: S]
- **`est_bytes` считается полным проходом под write-lock** — `src/meta_collector.rs:343-377`. O(объекты×колонки) держит write-guard, пока UI-читатели ждут; размер нужен лишь для строки лога. Считать ПОСЛЕ `drop(store guard)` либо инкрементально по added/removed (бюджет 1M, скан раз в 30с — масштаб скромный). [severity: low | усилия: S]
- **Перф-мелочи горячего/около-горячего пути (low):** `events.clone()` каждый кадр при фокусе (blink держит перерисовку) — `src/codeeditor.rs:946,1023` → ранний выход при `is_empty`; `eol_str.to_owned()` каждый кадр, нужен лишь в Paste/Enter — `src/codeeditor.rs:992-993`; `FontId` с аллокацией имени семейства + clone на токен — `src/highlight.rs:62-63,73`, `src/theme.rs:389-395` → `OnceLock`/`Arc<str>`; `max_cols` в байтах раздувает `total_w` (мнимый гориз. скролл на кириллице) — `src/codeeditor.rs:625-626`; `result_body` строит фиктивный `ResultSet`+`GridModel` ради `mem::replace` каждый кадр — `src/main.rs:2371-2372`; заголовки/флаги вкладок в новые `Vec<String>` каждый кадр — `src/main.rs:1813-1824,1123-1125`; грид результатов клонирует `Vec<String>` видимых строк/`row_values` каждый кадр — `src/main.rs:2380,2408`, `src/proc.rs:187-203` (виртуализирован — объём ограничен экраном); `meta_obj_sel.contains()` линейно на объект/кадр — `src/metadata.rs:536` → `HashSet`; `names` соединений пересобираются каждый кадр в connect_modal — `src/connections_ui.rs:213-217`; усечение заголовка вкладки каждый кадр — `src/menubar.rs:283-295` (ранний выход дешёв).
- **Перф-мелочи разового/фонового пути (low):** `context_text` вырезает ±400 строк на F6 — `src/complete.rs:81-88`; `.filter(Comment).collect()` повторно — `src/sqlfmt.rs:548,677,689,950,1096`; `push_str(&format!())` вместо `write!` в принтере — `src/sqlfmt.rs:1166-1271`; `resolve_values`/`formation_date` обходят поддерево события на каждое правило/событие (фоновый поток валидации) — `src/rules/mod.rs:419-460,257-289`; `build_index_and_chars` второй проход по starts — `src/doc/mod.rs:259-267`; `scan()`/`process_segment()` лишние проходы Vec<char> (фоновый поиск) — `src/search.rs:166-189`; `Nfa::closure` лишний `contains` перед sort/dedup — `src/xsd/model.rs:392-405`; `Nfa::expected` клон имён (только путь ошибки) — `src/xsd/model.rs:439-447`; `type_by_name` двойной lookup + panic-индексация — `src/xsd/loader.rs:129-133`; `parent.name.clone()` в путях ошибок — `src/validate.rs:356-389`; `col_left`/`col_at`/`insert_at` префиксные суммы на вызов — `src/grid.rs:173-193`; `truncate_to_width` линейное посимвольное измерение — `src/widgets.rs:958-980` (→ бинарный поиск); O(replies×tabs) разбор ответов — `src/metadata.rs:253-268`; `max_line_bytes` монотонно растёт — `src/doc/mod.rs:530-542` (задокументированная верхняя оценка скролла).

---

## Ложные тревоги / намеренные решения

- **Запаркованный SQL-форматтер/инспектор** — `src/sqlfmt.rs`, `connections::prepare_error`. UI убран 2026-06-14 намеренно («сделать по уму»), движок под `#[allow(dead_code)]`. НЕ удалять целиком — только чинить внутренние дефекты качества (например, high-находка про dollar-тег выше). Аналогично запаркован путь XML Run (`ProcKind::Run`/`ProcMsg::Tables`/`start_xml_run`/`shred.rs`) — намеренная парковка, не мусор (ветка `Tables` в `finish_proc` достижима через `shred::spawn_shred`).
- **Immediate-mode перерисовка каждый кадр** — egui перерисовывает всё каждый кадр by design; долексирование с бюджетом (`src/codeeditor.rs:111-145`) просит repaint до сходимости — намеренный ленивый механизм с гарантированной сходимостью (бюджет двигает states каждый кадр, переякорь при дальнем прыжке), инвариант advance соблюдён. Зависания нет.
- **`LineCache`** — `src/codeeditor.rs:38-67`. Двухпоколенный кэш с жёсткой границей ~2×CAP (swap+clear), неограниченного роста нет — это объём удерживаемой памяти, не утечка.
- **`depth` в минус при несбалансированных скобках** — `src/sqlfmt.rs:520-543, 416-440`. НЕ зависание: `check_parens` (1054) гейтит несбалансированный ввод в `parse_all:1300-1303` раньше `collect_until`/`take_parens`; `saturating_sub` — лишь необязательная страховка инварианта.
- **Свежее дерево зависимостей и его «дубли»** — все прямые версии актуальны; `quick-xml 0.39.4` и пр. — транзитивные платформенные (Wayland/Unix) дубли, в Windows-exe не попадают. Не находка.
- **`begin_document(attrs.iter().cloned().collect())`** — `src/validate.rs:437-440`. `attrs` нужен дальше (`node.attrs = attrs`, 451), `mem::take` сломал бы материализацию — клон единственно возможен и разовый (корень Document). Не дефект.
- **`grid.rs` wrap-ветка** — `src/grid.rs:365-373`. Самый дорогой участок недостижим в текущей сборке: оба `result_grid` зовутся с `wrap=false`, `mono_wrap_lines` под `#[allow(dead_code)]`. Учитывать лишь при будущей активации wrap.
- **«Дубли» now_ms / line-tracker / menu-стиля** — `connections.rs:204-208` берёт время из `m.created()`, `doc/mod.rs:46-48` — `as_nanos()` (разные источники/единицы); три line-tracker'а в format/validate/search решают разные задачи; блок стиля меню (`menubar.rs:28-48`) задаёт ИНЫЕ значения, чем `theme::apply` (это уникальный inline-стиль, не дубль темы). Унификация дала бы натяжку/отрицательную пользу.

---

## Предлагаемый порядок работ

**Этап 1 — быстрые победы (S, высокая отдача/риск):**
1. `sqlfmt.rs:186-193` — срез-сравнение вместо `collect::<String>()` (high, заодно через общий `dollar_close` закрыть дубль 1517-1537).
2. `close_tab` — выставлять `cancel` перед `remove(i)` (закрывает зомби-поток Format/Validate/Search + удержание mmap + осиротевший temp-файл одним движением); там же подключить `doc::cleanup_temp_dir` при старте.
3. `reset_session`/`kill_all` — слать `CancelRequest` для долгого SQL.
4. `complete.rs` — `last_prefix`-гейт для `ac_refilter`.
5. `highlight.rs:44-57` — `is_keyword` без аллокации (бинарный поиск/`OnceLock`).
6. Меню: пометить нереализованные пункты `disabled` (`menubar.rs`); попутно `fmt_elapsed`, `finished_word`, `menu_end=220.0`, `AcKind::PartialEq`, `_pos`/`_reader` — чистка мусора.

**Этап 2 — perf горячего пути (M):**
7. `get_line` второй клон в `hl_line` (`codeeditor.rs`) + ключ кэша без копии строки.
8. `char_pos` — кэш `char_base` строки (`doc/mod.rs:448`).
9. Дерево метаданных — кэш раскладки по `meta_view_gen` (`metadata.rs:485-549`) + `HashSet` для `meta_obj_sel`.
10. SQL-лексеры на `char_indices()` без Vec; `sqlfmt` номер строки через `partition_point`.

**Этап 3 — память и унификация (M):**
11. piece table: компактизация add + лимит undo (`doc/piece_table.rs`).
12. Унификация: `vbar`/`hbar` (axis-параметр), три кнопки (`filled_button`), `RunningProc::new`+`ProgressThrottle`, `data_sheet`/`dock_list_island`, `xsd_error`-хелпер, `to_wide`/`from_wide`, `reset_all_sessions`.

**Этап 4 — косметика и гигиена (S, по остаточному принципу):**
13. Сузить файловые `#![allow(dead_code)]` (widgets/dialog), удалить `qbtn_dim`, точечные allow.
14. Мелкие perf-аллокации каждого кадра (`events.clone`/`eol_str`/`FontId`/labels) и оставшиеся low-дубли по списку разделов 3/4/7.

Этапы 1–2 дают основной выигрыш по отзывчивости и ресурсам; 3 закрывает рост памяти и риск рассинхрона; 4 — гигиена кодовой базы.

---

### Дополнительно: вывод clippy (release)

- `src/sqlfmt.rs:1295` — «very complex type», вынести в `type`-алиас.
- `src/main.rs:362` — ручная реализация `.is_multiple_of()`, заменить на `(bytes.len() - i).is_multiple_of(3)`.

Остальное дерево компилируется чисто (грубого dead-code на уровне компилятора нет — серьёзные находки выше получены семантическим анализом).
