# Журнал прогресса

Новые записи — сверху. Каждая запись: что сделано, как проверено, цифры, что дальше, блокеры.

## ▶ Текущее состояние (сводка на 2026-11-08 вечер, читать первым)

**Где мы:** путь A (нативный просмотр данных) пройден — 6/6 вех. **Следующее по порядку:** Фаза R (реструктуризация репо, README, английский) → Фаза 6 **C2-b** (нативный рендер по состоянию живой игры). План — `docs/PLAN.md` (разделы «Фаза R», «Фаза 6 — C2-b»), решения — ADR 0004 (C2-b), 0005 (движок как фреймворк, подключение cargo git + `[patch]`). **Ничего из этого не начинать без команды пользователя.**

**Ветки, ждут мержа пользователем:** `feat/game-camera` в **обоих** репо — мержить вместе (игровые `er-tool`/`er-viewer` используют код ядра из этой ветки). Агент не мержит и не пушит.

**Раскладка:** `ProjectPort/` (не git) → `elder-grace/` (ядро, git), `grace-eldenring/` (игра, git), `.local/` (данные, префикс, кэши, кадры — не git). Чистая копия Elden Ring 1.17.2: `~/Games/EldenRing/Game` (read-only), песочница — `.local/game_bean/elden_ring`.

**Что работает (проверено):**
- **Эталон** на GPTK 3.0-3 одной командой: `cargo xtask oracle setup|steam [--ui]|run [--label L] [--no-shim] [--no-overrides] [--no-esync]|status|kill|shot <png>`. esync по умолчанию (+~20% в открытом мире). Steam логинится сам по сохранённой сессии.
- **Shim d3d12 в игре:** дамп шейдеров (5988), подмены на лету (`grace-ctl overrides on|off|toggle|reload`, `cargo xtask overrides --reload`; `.hlsl` для DXBC/DXIL, `.irpatch` — правка оригинального DXIL), `grace-ctl bound` (шейдеры кадра), камера и отладка памяти: `grace-ctl camera|peek|find-floats` (поиск по памяти процесса через VirtualQuery+ReadProcessMemory — безопасно). Активна только подмена «золотой текст»; демо-правки — `grace-eldenring/shaders/overrides/examples/`.
- **Замеры:** shim dxgi (время кадра), `cargo xtask bench`, `cargo xtask trace [--template "System Trace"]`.
- **Слой данных (`grace-eldenring/crates/er-data`):** RSA-ключи из `eldenring.exe` пользователя (в git нет), BHD5/BDT/AES, DCX (Kraken на чистом Rust), BND4, FLVER2, MATBIN, TPF, MSB. `er-tool keys|ls|find|probe|extract|flver-check|camera`.
- **Нативный рендер (`elder-grace/crates/grace-render`, `grace-eldenring/crates/er-viewer`):** Metal, инстансинг, BCn-текстуры, альфа-тест, камера (орбита + WASD), захват PNG, склейка «наш | эталон». Сцена Stranded Graveyard целиком (`er-viewer m18_00_00_00 --map`), ~1 с загрузки, 31 млн треугольников.
- **M6 — сравнение с эталоном автоматически:** игра в склепе → `cd grace-eldenring && cargo run -p er-tool -- camera --save cam.json` → `cd ../elder-grace && cargo xtask oracle shot oracle.png` → `cargo run --release -p er-viewer -- m18_00_00_00 --map --camera cam.json --capture ours.png --size 1728x972 --compare oracle.png`. Совпадение ~1–3% ширины кадра.

**Ключевые знания (подробно — `grace-eldenring/docs/FORMATS.md`, `BASELINE.md`):**
- Узкое место — CPU (главный поток под Rosetta + трансляция D3DMetal), GPU ~67%. Шейдеры игры на GPU уже нативные — свои MSL внутрь процесса игры не подсовываем.
- Камера: `cbSceneParam` (pos +48, proj +256, invView +320) в **локальных** координатах; мировые — через структуру камеры игры (матрица построчно + сдвиг начала, напр. (112, −8, −8) + [fovy, near, far, aspect]).
- **Нельзя:** подменять vtable ресурсов D3DMetal (игра падает), класть служебные слова ABI в копии vtable (падает). Копии vtable — прежняя раскладка (оригинал на [−1]).

**Пробелы рендера:** деревья SpeedTree (позиции в чужом потоке), кости статических мешей, нормали/свет/туман как в игре, текстуры-маски кусков карты, открытый мир (`m60`).

**Принципы:** легальная копия пользователя; pristine — только чтение; данные игры не в git; ничего онлайн; логины — пользователь; внешние сборки — с его «да»; крупное — в feat-ветках (коммитит агент, мержит пользователь); общение по-русски; после Фазы R — доки/код/коммиты на английском.

---

## 2026-11-08 — Сессия 7: камера из игры, M6 — ветки `feat/game-camera` (оба репозитория)

**Сделано:**
- `grace-proto`: `Request::Camera { layout }`, `CameraReport/CameraSample`, `camera_at` — самопроверяемая сигнатура (перенос «вид → мир» = положение или 0; поворот ортонормирован; проекция перспективная, fovy 10°..150°; экран — целые пиксели; обе ориентации матриц) + тесты; `Request::Peek` (чтение памяти игры) и список проекций для отладки без перезапуска.
- shim d3d12 (`resources.rs`): поиск по доступной на запись памяти процесса (VirtualQuery + ReadProcessMemory — безопасно), ~7 ГБ за ~12 с.
- `grace-ctl camera --layout … --save cam.json`, `grace-ctl peek <адрес>`; `cargo xtask oracle shot <png>` — снимок окна игры по заголовку.
- `grace-render`: `Camera::looking`, `compare::side_by_side` (с обрезкой полей); `er-viewer --camera cam.json [--offset x,y,z] --compare игра.png`.
- `grace-eldenring/data/camera-layout.json` — раскладка `cbSceneParam`.

**Что не сработало (важно не повторять):**
- Подмена vtable **ресурсов** D3DMetal (перехват Map) — игра падает при старте: D3DMetal опознаёт свои ресурсы по vtable.
- Копии vtable с сохранением служебных слов ABI ([-2], [-1]) и адресом оригинала на [-3] — тоже падение (вероятно, виртуальное наследование). Оставлена прежняя раскладка (оригинал на [-1]) — работает.
- `cbSceneParam` в upload-буферах GPU процессору через Windows-API не виден; находятся копии на стеке главного потока.

**Как проверено (пользователь в игре, 15:30–16:00):**
- Камера найдена в открытом мире и в склепе: fovy 48°, кадр 1728×972 — совпадает с видимой картинкой (поля окна); координаты локальные (|p| < 10).
- Склеп Stranded Graveyard у благодати `AEG099_060_9001`: наш кадр (Metal, нативно) и кадр игры с той же камеры — совпадают своды, арки, колонны, урны слева, саркофаг и статуя справа, факелы (`.local/captures/m6-final-vs.png`). Сдвиг локальная → карта (−114.62, 8.23, 6.86) подобран по кадру (сначала по позиции благодати, затем на глаз).

**Начало координат найдено (та же сессия):** второй проход (матрицы с тем же поворотом) нашёл мировую копию камеры, а `peek` вокруг неё — структуру камеры игры: матрица построчно + **сдвиг начала (112, −8, −8)** + [fovy, near, far, aspect]. `er-tool camera` (игровой репо) ищет её через `FindFloats` по fovy/aspect, читает `Peek`, мир = локаль − сдвиг, голосование копий (32/32). Ручной `--offset` больше не нужен; сравнение совпадает до ~1–3% ширины кадра (колонны, факелы, статуя, арка) — `.local/captures/m6c-ours-vs.png`.

**Воспроизвести (всё командами):** игра в склепе → `cd grace-eldenring && cargo run -p er-tool -- camera --save cam.json` → `cd ../elder-grace && cargo xtask oracle shot oracle.png` → `cargo run --release -p er-viewer -- m18_00_00_00 --map --camera cam.json --capture ours.png --size 1728x972 --compare oracle.png`.

**Дальше:** мерж веток `feat/game-camera` (оба репо вместе); затем — свет/туман/экспозиция из `cbSceneParam` (лежат там же), деревья SpeedTree, кости статических мешей; открытый мир по тайлам `m60` (камера уже работает и там).

---

## 2026-11-08 — Сессия 6: путь A — M1 и M2 (`grace-eldenring`)

**Решение:** после честного разбора («готов эталон и инструменты подмены, но игра целиком на трансляции, а шейдеры упираются в D3DMetal и GPU не узкое место») пользователь выбрал путь A — свой нативный рантайм поверх данных игры. Спека и план — в `grace-eldenring/docs/{specs,plans}/`.

**Сделано (workspace `grace-eldenring/`, крейты `er-data`, `er-tool`):**
- M1: RSA-ключи из `eldenring.exe` пользователя (в репозитории их нет), расшифровка и разбор всех 7 `.bhd` (~130 тыс. записей), хеш путей, индекс `Game::open/find`, `er-tool keys|ls|find`.
- M2: чтение `.bdt` + AES-128-ECB по диапазонам, DCX (KRAK — чистый Rust `oozextract`, DFLT, ZSTD), BND4, `er-tool extract|probe`.
- Найдено: Stranded Graveyard — MSB `m18_00_00_00` + два куска карты + ~4800 размещений ассетов `AEG…` (`/asset/aeg/…geombnd.dcx`, FLVER + LOD).

**Как проверено:** 15 тестов `er-data` (реальные файлы установки, пропуск без неё): все записи всех архивов укладываются в свои `.bdt`; MSB распаковывается (KRAK 159 КБ → 6.9 МБ, сигнатура `MSB `); msgbnd → 26 `.fmg`; извлечены FLVER куска карты и ассета `AEG001_290`. clippy чистый.

**Не сделано из плана:** дифференциальная сверка с `fromsoft-formats` (GPL, dev-dep) — отложена до FLVER, где ошибки вероятнее; форматы M1–M2 самопроверяются (границы, сигнатуры, размеры).

**Продолжение сессии 6 — первая картинка:**
- `er-data/flver.rs`: FLVER2 (версия 0x2001A) — материалы, меши, позиции/нормали/UV, индексы LOD0; тест: вершины в bbox, нормали единичные, индексы в пределах. В FLVER Elden Ring у текстур только имена слотов (`…_AlbedoMap`) — пути задаёт MATBIN материала.
- `elder-grace/crates/grace-render` (ядро): меши в Shared-буферах, глубина Memoryless, ламберт + небо, орбитальная камера, окно winit, `capture_png`.
- `grace-eldenring/crates/er-viewer`: модель из архивов → окно/PNG. Кадры (смотрел сам): AEG001_290 (растительность-карточки), **AEG003_055 — мешки на носилках**, AEG003_082 — палки с мешками; кусок карты `m18_00_00_00_000000` («暗室») — грубая служебная оболочка.

**M3/M4 (та же сессия):** `er-data`: MATBIN, TPF; `grace-render`: DDS → BCn как есть (все мипы), UV, сэмплер, альфа-тест; `er-viewer`: материал → MATBIN → альбедо (или путь из FLVER) → TPF. Кадры (смотрел сам): мешки на носилках, дрова с мешками, куст (BC7 + альфа) — с текстурами игры. Не покрыто: текстуры кусков карты `m18_…`, нормали/прочие слоты, освещение «как в игре».

**M5 (та же сессия):** `er-data/msb.rs` (349 моделей, 4683 части, 4533 ассета), `grace-render` — инстансинг и полёт WASD/QE, `er-viewer --map`: 4535 частей → 977 мешей, 122 текстуры, 11 054 экземпляра, 31 млн треугольников, загрузка ~1 с (release). Трансформ части T·Ry·Rz·Rx·S — стены сходятся, значит порядок верный. Кадры (смотрел сам): готические стены с арками, своды, коридоры, колонны с текстурами игры.
Известные пробелы: деревья SpeedTree (`AEG801_*`: позиции в чужом потоке, 31 меш не рисуется), 4 модели `AEG099_*` не найдены, у `AEG099_720` вершины в пространстве костей (не применяем кости), текстуры кусков карты `m18_00_9xx` (маски без альбедо), свет/туман/нормали как в игре.

**Дальше:** M6 — сравнение с эталоном требует камеру из живой игры → отдельная ветка инъекции (решение пользователя). Без игры можно: деревья SpeedTree, кости статических мешей, нормали.

---

## 2026-11-08 — Сессия 5: разбор ожиданий → esync (ветка `feat/dxil-overrides`)

**Сделано:**
- `cargo xtask trace --template "System Trace" [--instrument I]` (коммит `5609afb`).
- Трейс 15 с в открытом мире (пользователь в игре), выгрузка `thread-state` (2,2 ГБ XML) и `time-profile`, свод скриптами (разовые, в scratchpad; при повторе — перенести в `grace-bench`).
- Находка: главный поток 66% работает / 24% заблокирован / 10% ждёт ядро (на Super-ядрах); **22% его рабочего времени — `wine_server_call`** (`NtSetEvent`, `NtResetEvent`, `NtReleaseMutant`, `NtCreateEvent`), у пяти рабочих потоков — 45–59%. Подробно — `grace-eldenring/docs/BASELINE.md`.
- В `ntdll.so`/`wineserver` GPTK собран esync → `oracle`: `WINEESYNC=1` по умолчанию, `--no-esync`, проверка режима запущенного wineserver (иначе Wine отказывается запускать процесс), `status` показывает режим (коммит `8c846a7`).

**Как проверено:** A/B у места благодати, 20 замеров по секундам: без esync 49.6 FPS (46.8–54.6) → с esync 59.1 (55.0–60.0, лимит игры), Steam и игра с esync стартуют без ошибок. Ракурс во втором замере тяжелее (1744 отрисовки против 903), так что +19% — нижняя оценка. Открытый мир, 298 с бега: **47.1 FPS против 38.7** у world-base, p95 **26.6 против 40.0 мс** (маршруты разные); трейс: GPU 67%, главный поток 80% ядра.

**Дальше:**
- ~~Переснять базовую линию с esync в открытом мире~~ — сделано (`world-esync`), см. BASELINE. Следующий разбор: System Trace с esync — на что уходят 80% главного потока (Wine unix-код, D3DMetal, Rosetta) и оставшиеся ожидания.
- Если упор в лимит 60 мешает мерить — найти штатный для сообщества способ снять ограничение FPS офлайн (только песочница) или мерить запас по GPU/CPU-времени кадра.
- Не проверено: msync (у CrossOver) против esync — в Wine GPTK msync нет.

---

## 2026-11-08 — Сессия 4: DXIL-подмены — ветка `feat/dxil-overrides` (поверх `feat/live-overrides`)

**Сделано:**
- dxc v1.9.2609 собран из исходников (разрешение пользователя «Собирать dxc»), `dxc`/`dxa`/`dxv` + `libdxcompiler.dylib` в `.local/tools`; рецепт — ARCHITECTURE, лицензия — THIRD_PARTY.
- `grace-shader`: сигнатуры ISG1/OSG1 (DXIL), отладочное имя из `ILDN`, `names <дамп> [подстрока]`; тест на синтетическом контейнере.
- `xtask/src/overrides.rs`: `.hlsl` → vkd3d или dxc по типу оригинала; **`.irpatch`** — правка оригинального DXIL без исходников (dumpbin → `storeOutput` × константа → dxa → dxv); сверка сигнатур; удаление устаревших подмен; `cargo xtask shader <id> [--ir]`. 2 теста патчера.
- shim d3d12 + `grace-ctl bound [--ms] [--ps]` — какие шейдеры привязаны за окно (по рецептам PSO).
- `grace-eldenring`: `shaders/overrides/1f57f44ecec2766b.ps.irpatch` — холодный оттенок у `ToneMap_TabledToneMapPS` (коммит в `main` игры `4b832c3`); FORMATS — отладочные имена шейдеров.

**Как проверено (без игры):**
- dxc читает DXIL игры (`-dumpbin`), HLSL → DXIL подписан (хеш ненулевой); тестовый DXIL и **пропатченный тонмаппинг игры** проходят Metal Shader Converter (`grace-irconv`) и нативно грузятся в Metal (M5 Pro, 1/1). Тот же конвертер использует D3DMetal — ожидаю, что игра примет.
- Круг «дизассемблер → dxa → dxv» на тонмаппинге: `Validation succeeded`, сигнатуры совпали.
- `cargo test -p grace-shader -p xtask -p grace-proto` зелёные, clippy по изменённым крейтам чистый.

**Проверено в игре (2026-11-08, 13:10–13:30, Steam вошёл сам по сохранённой сессии):**
- Титульный экран: `grace-ctl bound` — ~6 PSO; тонмаппинга там нет, есть DXIL `GXSimple_BlendUI_SDR2SDR_F` (последний проход: интерфейс поверх кадра).
- `.irpatch` на него (красный) + на тонмаппинг (холодный) → `cargo xtask overrides --reload` в живой игре: «PSO пересоздано 5, ошибок 0» — **D3DMetal принимает пропатченный DXIL**. В мире (пользователь играл, катакомбы) скриншоты `off`/`on`: обычный кадр ↔ весь кадр с интерфейсом в красно-коричневом тоне (обе правки). Пар PSO — 8, «не создано» — 0.
- Не разобрано: на титульном экране правка `BlendUI` видимого эффекта не дала (снимок сразу после reload, режим Direct).
- **A/B цены перехвата** в мире: 6 циклов × (10 с on, 10 с off), FPS из shim dxgi раз в секунду (без первых 2 с окна), по 48 замеров: on — 49.6 FPS (медиана 49.3), off — 46.2 (47.0). Разница — от смены сцены (игрок двигался), **цены перехвата `SetPipelineState` на фоне шума не видно**. Строгий замер — два прогона по маршруту и `bench`.
- Демонстрационные DXIL-правки перенесены в `grace-eldenring/shaders/overrides/examples/` (не собираются по умолчанию), активна только «золотой текст» (в мире — текст HUD, 7 PSO).

**Дальше (предложение):** мерж веток; затем первые полезные подмены вместо оттенков — кандидаты по профилю GPU (что дорогое в кадре мира: `cargo xtask trace` + `grace-ctl bound`), и разбор ожиданий CPU/GPU (отложено, нужен прогон в мире).

---

## 2026-11-08 — Сессия 3: подмены на лету — ветка `feat/live-overrides`

**Сделано** (2 коммита агента в ветке, не смержено):
- `19e7f4f` — пары PSO «оригинал → подменённый», копия vtable командных списков (`SetPipelineState` 25, `Reset` 10, `ClearState` 11, `QueryInterface` 0; `CreateCommandList` 12 и `CreateCommandList1` 51 — сверено с `d3d12.h` mingw-w64), канал управления TCP 47322, `grace-proto` `Overrides{op}` + `OverrideStatus`, `grace-ctl overrides`. Подменённым описаниям обнуляется CachedPSO; если подменённый PSO не создался, игра работает с оригиналом (раньше получила бы ошибку).
- `aecdf3a` — рецепты PSO (`recipes.rs`) и `overrides reload`, `cargo xtask overrides --reload`.

**Как проверено (сам, в игре, титульный экран, `oracle run --label live-ov-2`):**
- `grace-ctl overrides`: файлов 1, пар PSO 1, рецептов 8888, подстановки растут (~30/с).
- `off` → счётчик подстановок стоит, текст «НАЖМИТЕ ЛЮБУЮ КНОПКУ» белый; `on` → снова золотой и счётчик растёт (скриншоты `screencapture`, сам смотрел).
- Перекрасил HLSL в голубой → `cargo xtask overrides` + `reload` → текст голубой (пересоздание 89 мс, первый раз с чтением дампа); вернул файл → `reload` → золотой (2 мс). Убрал `.dxbc` подмены → `reload` → «снято 1», пар 0; вернул → пара снова.
- clippy по изменённым крейтам чистый, тест протокола (`overrides_wire_format`) зелёный.

**Не проверено / ограничения:**
- Цена перехвата `SetPipelineState` в мире (тысячи вызовов на кадр) — нужен прогон пользователя: `oracle run --label ov-on`, `grace-ctl overrides off` на середине или два прогона, `cargo xtask bench`.
- Бандлы, записанные до переключения, сохраняют прежний PSO до перезаписи игрой.
- Рецепты держат ссылку на все ~8900 PSO (утечка, если игра пересоздаёт пайплайны; у Elden Ring библиотека грузится один раз) — только при заданном `GRACE_SHADER_OVERRIDE`.
- Список `applied` в статусе — подмены, применённые хоть раз (после снятия не чистится).
- Не связано с веткой: `cargo clippy --workspace` даёт 13 предупреждений `transmute used without annotations` в `tools/grace-irconv` (новый lint тулчейна, код не менялся); репозиторий в целом не отформатирован `rustfmt` (251 diff ещё до ветки) — отдельная мелкая задача в `main`.

**Дальше:** мерж пользователем → `feat/dxil-overrides` (сборка dxc — нужно «да» пользователя на сборку чужих исходников) → подмены DXIL-шейдеров (99,5% корпуса). Отложенный разбор ожиданий — с пользователем.

---

## 2026-11-08 — Сессия 2: замеры (фаза 1), инструменты — в main

**Сделано** (коммиты в `main` движка, сделаны агентом с разрешения пользователя):
- `shim/dxgi-proxy` → `dxgi.dll`: перехват `Present`/`Present1` через копии vtable фабрики и swapchain, журнал времени кадра CSV, статистика по TCP 47321.
- `crates/grace-bench` + `cargo xtask bench`: отчёт по журналу (FPS, p50/p95/p99, 1%/0.1% low, статтеры, гистограмма) и сравнение двух прогонов.
- `grace-ctl --shim stats|watch` (TCP к shim в игре).
- `cargo xtask trace`: Metal System Trace запущенной игры + разбор (занятость GPU, CPU по потокам, вердикт).
- `oracle run --label` (метка прогона), `--no-shim` теперь убирает только d3d12-прокси.

**Как проверено / цифры (только меню — не базовая линия):**
- журнал: 5743 кадра за 134 с, средний 42.9 FPS, p95 30.7 мс, 1% low 6.0 (загрузки и переходы меню);
- трейс меню (окно, возможно, в фоне): GPU занят 28–35%, главный поток 55–64% ядра, процесс ~2.9 ядра → вердикт «ожидание», ни GPU, ни CPU не насыщены;
- 5 тестов grace-bench, clippy чистый.

**Дальше (нужен пользователь):** базовая линия в мире — маршрут 1–2 мин, `cargo xtask oracle run --label world-base`, во время прогона `cargo xtask trace --label world-base`, затем `cargo xtask bench world-base`. Окно игры должно быть активным (в фоне macOS режет частоту). Затем — ветки `feat/live-overrides` и `feat/dxil-overrides`.

---

## 2026-11-08 — Сессия 1: shim внутри Elden Ring, первые шейдеры игры в нативном Metal

**Сделано (фазы 2–3, первые шаги)**
- `shim/graphics-proxy` → `d3d12.dll`: прокси D3DMetal внутри процесса игры (ординалы как у Windows, override `d3d12=native,builtin` для exe игры в `oracle setup`). Перехват создания пайплайнов через **копию vtable** устройства (оригинальная vtable D3DMetal в read-only памяти маковского фреймворка, `VirtualProtect` Wine её не меняет — первая попытка молча не сработала). Дамп шейдеров в `.local/shaders/eldenring/`.
- `crates/grace-shader`: разбор контейнеров DXBC/DXIL, `inventory`, `info`, `metallib` (нативная загрузка в Metal). 3 теста (синтетический контейнер, мусор, реальный корпус при наличии).
- `tools/grace-irconv` (x86_64-apple-darwin, под Rosetta): DXIL → metallib через `libmetalirconverter.dylib` из GPTK.
- `cargo xtask shaders [игра]` — сводка → конвертация → проверка одной командой. `oracle`: переменная `GRACE_WINEDEBUG` для каналов Wine; `status` видит процесс игры.

**Как проверено / цифры**
- Лог shim: `D3D12CreateDevice(0xc000) → 0`, `CreatePipelineLibrary(451014 байт)`, пайплайнов graphics 46 · stream 1.
- Корпус меню: 54 шейдера (DXBC 5.0: 11 ps + 18 vs; DXIL 6.0: 11 ps + 12 vs + 1 hs + 1 ds), 371 КБ, нераспознано 0.
- DXIL → metallib: 25/25; Metal (Apple M5 Pro): библиотек загружено 25/25, функций 29, ошибок 0.
- clippy чистый, все тесты зелёные. Игра падений не дала; упал только оверлей Steam (`gameoverlayui`: `ClientAPI_Init` assert → падение в dbghelp) — для эталона не нужен.

**DXBC-ветка (после разрешения пользователя на сборку vkd3d):**
- vkd3d 1.19 собран в `.local/tools` (рецепт и подводные камни — `ARCHITECTURE.md`: старый системный bison, нужен полный `make`, `configure` требует MoltenVK).
- `cargo xtask shaders`: DXIL 25/25 через Metal Shader Converter + DXBC 29/29 через vkd3d → SPIRV-Cross → `xcrun metal`. **Итог: Metal (Apple M5 Pro) — библиотек 54/54, функций 58, ошибок 0.**
- Это компилируемость, не рендер: привязки ресурсов D3D12 → Metal ещё не сопоставлены.

**Базовая линия (на глаз, пользователь):** ~40 FPS в открытом мире и подземельях, ~35 на максимальных настройках; shim на FPS заметно не влияет (`oracle run` против `oracle run --no-shim`). Записано в `grace-eldenring/docs/BASELINE.md`.

**Библиотека пайплайнов:** в мире новых шейдеров почти не было, потому что игра грузит PSO из `ID3D12PipelineLibrary`. В shim добавлена копия vtable библиотеки (слоты 9/10/13: `LoadGraphicsPipeline`, `LoadComputePipeline`, `LoadPipeline`). Результат одного запуска до меню: из библиотеки 8874 PSO, корпус 5988 шейдеров (106 МБ), 99,5% — DXIL 6.0. Сводка `_stats.txt` теперь пишется каждые 25 пайплайнов.

**Полный корпус через конвейер** (`cargo xtask shaders`, лог `.local/logs/shaders-full.log`): DXIL → metallib 5957/5957, DXBC → metallib 31/31, **Metal (Apple M5 Pro): библиотек 5988/5988, функций 6164, ошибок 0.** Это компилируемость; привязки ресурсов D3D12 → Metal по-прежнему не сопоставлены.

**Подмена шейдеров (C1), первая версия:**
- Ресурсные привязки решены «по-хорошему»: подменяем байткод на уровне D3D12, а Metal-код из него делает сам D3DMetal; корневая сигнатура и состояние PSO остаются игровыми. Наш metallib напрямую в D3DMetal не подложить — точки входа нет.
- `shim/graphics-proxy/src/overrides.rs`: каталог `GRACE_SHADER_OVERRIDE` (`<fnv64>.<стадия>.dxbc`), подмена в копии описания (graphics 656 Б, compute 56 Б, поток подобъектов), загрузка подменённого PSO из библиотеки отдаётся как промах (`E_INVALIDARG`).
- `grace-shader info` печатает сигнатуры ISGN/OSGN (семантики, регистры, маски).
- Первая подмена — шрифт интерфейса `cfb409a01172e9fb.ps` (DXBC ps_5_0): HLSL в `grace-eldenring/shaders/overrides/`, та же альфа глифа, цвет — золотой. `cargo xtask overrides` собирает HLSL через vkd3d; `oracle run [--no-overrides]`.
- Проверено: лог shim `подмена шейдера cfb409a01172e9fb.ps применена`, игра работает, новых падений нет. **Пользователь подтвердил: текст меню золотой — первый кадр Elden Ring, часть которого нарисована нашим шейдером.**

**Блокеры / не сделано**
- Корпус снят только со стартового меню; для мира нужно зайти в игру (`cargo xtask oracle run`, загрузить сохранение) — пайплайнов будут тысячи.
- Не проверено пользователем: Cmd+Tab после `CaptureDisplaysForFullscreen = n`.

**Дальше:** (1) фаза 1 — журнал времени кадра из shim (перехват Present), маршрут, Metal System Trace: CPU- или GPU-bound; (2) подмены для DXIL-шейдеров (99,5% корпуса) — нужен компилятор HLSL→DXIL (dxc) или правка DXIL; (3) переключение подмен на лету через `grace-ctl` для A/B; (4) коммит.

---

## 2026-11-08 — Сессия 0f: CEF Steam под GPTK поднят

**Сделано**
- `shim/crash-probe` (Windows-exe): AeDebug-отладчик вместо отсутствующего в GPTK `winedbg`. Регистрация: `HKLM\Software\Microsoft\Windows NT\CurrentVersion\AeDebug` → `Debugger = C:\grace\crash-probe.exe %ld %ld`, `Auto = 1`. Отчёты в `C:\grace-crash\<pid>-<tid>.txt`: исключение как `модуль+оффсет`, регистры, адреса возврата (эвристика), **строки рядом с падением**, модули.
- Причина падения `steamwebhelper` найдена по строкам из отчёта: `int3` в `libcef.dll+0x59ef905` — фатальная проверка Chromium, которой нужен `QueryUnbiasedInterruptTimePrecise` из `api-ms-win-core-realtime-l1-1-1.dll` (Win10+). В Wine 7.7 этих функций нет; Chromium ищет их через `GetModuleHandle(<имя API-set>)`, а Wine разрешает имя в свой `kernelbase`.
- `shim/compat-realtime` → `api-ms-win-core-realtime-l1-1-1.dll`/`-l1-1-2.dll` в `system32` префикса: `Query*InterruptTime*`, `Query*CycleTime*`, aux-counter → `E_NOTIMPL`.
- `compat-bcrypt` (его импортирует `steamwebhelper.exe`) теперь подгружает compat-realtime и ставит **запасной `GetProcAddress`** в IAT `libcef.dll`, `steamwebhelper.exe`, `chrome_elf.dll`: если Wine вернул NULL — функцию ищем в compat-DLL (`src/getproc_fallback.rs`).
- Shim-DLL получили свои базовые адреса (`build.rs`, `--image-base` 0x6f0000000/0x6f1000000/0x6f2000000): дефолтный `0x180000000` занят `libcef.dll`.
- Отладочные хаки откатаны: AppInit_DLLs выключен, ключи relay-трассы удалены.

**Как проверено:** `steamui_html.txt` → `CreateResponse` / `BrowserReady`, `webhelper` больше не перезапускается, `crash-probe` отчётов не пишет. Окно Steam запущено для входа пользователя.

**Ошибка транспорта 0x3007** (окно «Непредвиденная ошибка транспорта» при запуске Steam):
- Причина: клиент принимает соединение `steamwebhelper` через `AcceptEx`, затем зовёт `getsockname` на принятом сокете; Wine 7.7 не знает локальный адрес AcceptEx-сокетов → `WSAEINVAL` (`net.cpp: getsockname failed in BGetBoundAddr with error 10022`). `SO_UPDATE_ACCEPT_CONTEXT` Wine игнорирует, а Steam ещё и передаёт в него сам принятый сокет.
- Исправление в `compat-bcrypt` (`src/iat.rs` — общий IAT-патчер по имени/ординалу, цель `*` = все модули, плюс патч при загрузке через `LdrRegisterDllNotification`; `src/winsock.rs`): обёртка `WSAIoctl` отдаёт вместо `AcceptEx` свою, которая запоминает «принятый → слушающий»; `getsockname` при `WSAEINVAL` отдаёт адрес слушающего (как Windows). Хуки ws2_32 по ординалам 3/6/21 и имени `WSAIoctl`.
- Проверено: 16 подмен адреса за запуск, новых `BGetBoundAddr` — 0, `BrowserReady`, отчётов `crash-probe` нет.

**Ошибка транспорта 0x3009:**
- Причина: клиент проверяет владельца websocket-подключения через `GetExtendedTcpTable(AF_INET, TCP_TABLE_OWNER_PID_CONNECTIONS)`, а Wine 7.7 на macOS отдаёт таблицу пустой (проверено `load-probe.exe tcp-owners`: 0 строк даже для своего loopback-соединения) → `transport_client.txt`: `Connection rejected`.
- Исправление `compat-bcrypt/src/tcp_owners.rs`: реестр TCP в общей памяти префикса (`Local\ElderGraceTcpOwners`, 1024 слота). Хуки `connect`/`listen` (ws2_32, по ординалу и по имени) пишут строки с PID; `closesocket` удаляет; хук `GetExtendedTcpTable` (классы 3/4/5, IPv4) дописывает строки реестра к ответу Wine, пропуская умершие процессы.
- Проверено: `GetExtendedTcpTable(class 4): Wine 0 строк + реестр 2`, новых `Connection rejected` — 0, **появился экран входа Steam** (подтвердил пользователь).

**Вход и первый запуск игры:**
- Пользователь вошёл в окне Steam: `connection_log` → `Logged On` (сессия запомнена). В `-silent` главного окна нет — это нормально.
- Elden Ring из песочницы (`steam_appid.txt` = 1245620, `bcrypt.dll` shim рядом, override `eldenring.exe`) при залогиненном Steam: процесс живёт > 1.5 мин, CPU ~135%, 3–5 ГБ RSS, `crash-probe` отчётов нет. Что на экране — подтверждает пользователь.

- **Пользователь подтвердил: стартовое меню Elden Ring отрисовано под GPTK.** В мир не заходил. Проблема: из полноэкранной игры нельзя было уйти (Cmd+Tab) — Wine захватывает дисплей; выставлен `HKCU\Software\Wine\Mac Driver\CaptureDisplaysForFullscreen = n` (не проверено пользователем).

**`cargo xtask oracle`** (`xtask/src/oracle.rs`): `setup | steam [--ui] | run [игра] [--no-hud] | kill | status`. В `GameConfig` добавлены `exe`, `steam_appid` (xtask не знает об играх); `[oracle] env = "gptk"`. Проверено: `setup` на существующем префиксе идемпотентен (< 1 с после сборки), `status` → `Steam: в сети`, 0 отчётов crash-probe; clippy чистый, тесты зелёные. **Не проверено:** `setup` на чистом префиксе с нуля и `run` через команду (игру запускали вручную).

**Дальше:** пользователь пробует `cargo xtask oracle run` и Cmd+Tab → фаза 1 (маршрут в мире, Metal HUD, xctrace, frame capture) → проверка `oracle setup` на чистом префиксе.

---

## 2026-11-07 — Сессия 0e: Steam под GPTK, compat-bcrypt

**Сделано**
- Причина падения Windows-Steam под GPTK: `BCryptGenRandom` с псевдо-хендлом `0x81` (Win10+), builtin `bcrypt.dll` Wine 7.7 разыменовывает его (падение в `bcrypt.dll+0x390A`, внутри `BCryptGenRandom`). Статически его импортируют `steamclient64.dll` и `tier0_s64.dll`.
- Написан `shim/compat-bcrypt` (→ `bcrypt.dll`): 61 экспорт naked-трамплинами в builtin bcrypt, перевод псевдо-хендлов 0x01–0xf1 через `BCryptOpenAlgorithmProvider` с кэшем. `load-probe.exe bcrypt-rng`: без shim — page fault, с shim — `status 0x0` и случайные байты. `cargo xtask shim` собирает его.
- Установлен в `Steam/bcrypt.dll` + `AppDefaults\steam.exe\DllOverrides bcrypt=native,builtin` (также для `steamservice.exe`, `eldenring.exe`).
- **Результат:** ядро клиента Steam больше не падает — самообновление проходит, сеть подключается (`connection_log`: Connected).

**Блокер:** интерфейс Steam (CEF, `steamwebhelper.exe`) не поднимается на Wine 7.7:
- без флагов webhelper перезапускается каждые ~10 с, `steamui_html.txt`: бесконечный `RetryCreateBrowser`, явных падений нет;
- с `-no-cef-sandbox -cef-disable-gpu` webhelper падает: `int3` в `libcef.dll+0x59ef905` (вероятно CHECK в Chromium) и исполнение по адресу данных в `libpyrowave-shared-1.dll`;
- shim из каталога CEF убран (там он не нужен и мешал разбору), оставлен только рядом с `steam.exe`.
- Без UI нельзя ввести логин и Steam Guard. Варианты: (A) логин через Windows-SteamCMD в том же префиксе и переиспользование токена клиентом в `-silent`; (B) копать CEF дальше; (C) CrossOver. Решает пользователь.

**Вариант A (выбран пользователем), результат:**
- Windows-SteamCMD (`C:\steamcmd`, официальный zip Valve) после бутстрапа тоже падал на псевдо-хендле — `compat-bcrypt` + override `steamcmd.exe` починили. Пользователь залогинился сам (пароль + Steam Guard): `Logged On`.
- `steam.exe -silent -no-browser`: клиент подхватывает аккаунт (`SetSteamID`), но **вход не начинает** — в новом клиенте вход запускает UI (CEF), а webhelper всё так же падает/перезапускается.
- Elden Ring при запущенном (но не залогиненном) клиенте прожил ~50 с и упал: page fault на исполнение по `0x138FBF084` (модуль не определён; возможно, следствие незалогиненного Steam).
- **Итог по GPTK:** shim, compat-shim'ы и SteamCMD работают; Steam-клиент на Wine 7.7 без рабочего CEF не логинится. Дальше: либо чинить CEF под Wine 7.7, либо CrossOver для эталона.

---

## 2026-11-07 — Сессия 0d: разделение репозиториев, Metal Toolchain, GPTK

**Сделано**
- Репозитории разделены (ADR 0003): `ProjectPort/elder-grace` (этот, ядро) и `ProjectPort/grace-eldenring` (доки игры перенесены туда из `docs/games/elden-ring/`, `git init`, свой `CLAUDE.md`). `.local/` осталась общей в `ProjectPort/`. В `ProjectPort/CLAUDE.md` — указатель (не под git). Ссылки в доках ядра переписаны на `../grace-<игра>/docs/`.
- Из индекса `elder-grace` убраны (`git rm --cached`) только перенесённые `docs/games/elden-ring/*`; остальное, что пользователь добавил в индекс, не трогали. Коммитов нет.
- Metal Toolchain: после `sudo xcodebuild -runFirstLaunch` (сделал пользователь) — `xcodebuild -downloadComponent MetalToolchain`, `xcrun metal` → Apple metal 32023.922.
- GPTK: пользователь выбрал Apple GPTK. Ставим cask `gcenx/wine/game-porting-toolkit` 3.0-3 (Wine + D3DMetal; `brew trust gcenx/wine` — с согласия пользователя; cask снимает карантин и переподписывает приложение ad-hoc).

**Как проверено**
- После переноса: `cargo clean && cargo test` — зелёные (старый тестовый бинарник падал из-за пути, вшитого через `CARGO_MANIFEST_DIR` до переноса — лечится пересборкой); `cargo xtask bundle` и `cargo xtask shim` собираются.

**Заметка:** RustRover-проект (`.idea/`) лежит в `ProjectPort/`; после переноса Cargo-проект надо подключить из `elder-grace/Cargo.toml`.

**GPTK и первый запуск эталона**
- GPTK установлен: `/Applications/Game Porting Toolkit.app`, D3DMetal 3.0, `wine64` (сообщает «wine-7.7 (Game Porting Toolkit 1.1)» — строка версии старая, сборка 3.0-3).
- Префикс: `ProjectPort/.local/prefixes/eldenring` (`wine64 wineboot --init`, ~14 с).
- Добавлен `shim/load-probe` (Windows-exe): `wine64 load-probe.exe graphics_proxy.dll` → «shim загружен, протокол 1», exit 0. **Shim грузится внутри Wine — мост к фазе 2 проверен.** `cargo xtask shim` собирает оба.
- Elden Ring из песочницы стартует, но выходит: нужен Windows-клиент Steam в префиксе (подробности — `grace-eldenring/docs/README.md`, раздел «Эталон»). В песочницу положен `steam_appid.txt` (1245620).

**Блокер:** логин Windows-Steam в префиксе требует Steam Guard. Пользователь должен получить доступ к кодам (мобильное приложение Steam или код восстановления), после чего: ставим Steam в префикс, он логинится один раз, дальше Steam в офлайн-режиме.

**Дальше:** Steam в префикс → запуск ER через него → фаза 1 (маршруты, Metal HUD, xctrace, frame capture).

**Обновление:** Windows-Steam поставлен в префикс GPTK (`.local/downloads/SteamSetup.exe`, официальный CDN Valve, sha256 `7d365453…12bcb`), но после самообновления 64-битный клиент падает на старте под Wine 7.7 из GPTK. **GPTK как среда эталона для Steam-игр не годится.** Предложено перейти на CrossOver (новый Wine + D3DMetal; там Elden Ring у пользователя уже работал). GPTK остаётся инструментом (D3DMetal, Metal Shader Converter) и проверенным хостом для shim.

---

## 2026-11-07 — Сессия 0c: окружение и скелет workspace

**Сделано**
- Окружение: Rust 1.99.1 (`rustup update`), таргет `x86_64-pc-windows-gnu` + mingw-w64 (GCC 16.2), .NET 10.0.403 (`brew install dotnet`; для внешних инструментов может понадобиться `DOTNET_ROOT=/opt/homebrew/opt/dotnet/libexec`), SteamCMD.
- Cargo workspace (см. «Что есть сейчас» в `ARCHITECTURE.md`): `grace-core`, `grace-proto`, `grace-ctl`, `grace-app`, `xtask`, `shim/graphics-proxy`.

**Как проверено**
- `cargo clippy --all-targets` — без предупреждений; `cargo test` — 2 теста зелёные (пример конфига разбирается, протокол туда-обратно).
- `cargo xtask bundle` → `target/Grace.app`, ad-hoc подпись. Запуск: окно, Metal на «Apple M5 Pro», **120 FPS** (ProMotion), drawable 2560×1600.
- `grace-ctl ping` → `pong: grace-app 0.1.0 (протокол 1)`; `grace-ctl stats` отвечает с живого окна.
- `grace-app --capture .local/captures/phase0-triangle.png --size 1280x800` — PNG открыт и просмотрен: треугольник с градиентом на тёмном фоне, как задумано.
- `cargo xtask shim` → `graphics_proxy.dll`, PE32+ x86-64 DLL, экспорт `grace_shim_version` есть.
- Скриншот экрана (`screencapture`) из сессии агента не работает — нет разрешения на запись экрана; для проверки кадров используем `--capture`.

**Блокеры**
- **Metal Toolchain** не ставится: Xcode не прошёл первую настройку (нет CoreSimulator). Нужен `sudo xcodebuild -runFirstLaunch` (делает пользователь), потом `xcodebuild -downloadComponent MetalToolchain`. Пока шейдеры компилируются в рантайме — для фазы 0–1 этого хватает.
- **CrossOver не установлен**: в `~/Applications/CrossOver/` остались только ярлыки от старой установки, приложения и бутылок нет. Нужна среда эталона — CrossOver или Apple GPTK. Решает пользователь.
- Вопрос пользователя о разделении репозитория (порт C и движок) — см. ответ в сессии; пока монорепо.

**Дальше**
1. Пользователь: `sudo xcodebuild -runFirstLaunch`; выбрать и поставить среду эталона.
2. Офлайн-запуск Elden Ring из песочницы в эталоне → фаза 1 (маршруты, Metal HUD, xctrace).
3. Проверить, что `graphics_proxy.dll` грузится в Wine (DLL override) — мост к фазе 2.

---

## 2026-11-07 — Сессия 0b: чистая копия Elden Ring

**Сделано**
- SteamCMD поставлен (`brew install --cask steamcmd`), но вход упёрся в Steam Guard. Скачали через консоль уже залогиненного клиента Steam (`download_depot`), депо 1245621, 1245624, 2778580. Депо 1245622/1245623 на лицензию не выдаются — региональные, не нужны.
- Собрано в `~/Games/EldenRing/Game` (перемещением, без копий), сделано read-only. Песочница — APFS-клон в `.local/game_bean/elden_ring`. Создан `config/local.toml`.
- Проверено: 64 файла, все `Data*.bhd/.bdt`, DLC, `regulation.bin`, `oo2core_6_win64.dll`; exe x86-64 ProductVersion 2.7.2.0; заголовок BHD зашифрован. Запись в чистую копию падает с `Permission denied`.

**Дальше:** фаза 0 — окружение (Metal Toolchain, `rustup update`, .NET, кросс-таргет), офлайн-запуск эталона из песочницы в CrossOver, скелет workspace.

---

## 2026-11-07 — Сессия 0: концепция и документы

**Сделано**
- Обсудили исходные `CLAUDE.md`/`PLAN.md` (движок только под Elden Ring, без Wine, C#+C++). Пересобрали концепцию:
  - проект — инструмент для портирования любых игр, Elden Ring — первая тестовая цель (предварительно);
  - эталон под CrossOver/GPTK + режим C как площадка + режим A как долгий путь (ADR 0001);
  - стек Rust + Metal через objc2 (ADR 0002).
- Разобрали Horizon (Minecraft на Rust): взяли идеи подготовки ресурсов, дифф-харнессов, `ctl`+MCP (см. `REFERENCES.md`).
- Написаны: `CLAUDE.md`, `README.md`, `docs/PLAN.md`, `docs/ARCHITECTURE.md`, `docs/REFERENCES.md`, `docs/THIRD_PARTY.md`, ADR 0001–0002, `docs/games/README.md`, `docs/games/elden-ring/{README,FORMATS}.md`, `.gitignore`, `config/local.example.toml`.
- `git init` в `ProjectPort` (отдельный репозиторий; домашний каталог пользователя тоже под git — не путать). Коммитов пока нет.

**Окружение (замерено)**
- macOS 27.3 (26C5035r), Xcode 27.2 (27B52), Apple M5 Pro, 48 ГБ.
- Rust 1.87.0 / cargo 1.87.0 через rustup 1.28.2 — **устарел, нужен `rustup update`**.
- CMake 4.4.3, Homebrew есть.
- **Нет:** Metal Toolchain (`xcrun metal` → «missing Metal Toolchain»), .NET SDK, SteamCMD.

**Блокеры**
- Конфиг Mac друга (M4) неизвестен — спросить.
- IW4L: ссылки нет — спросить или найти.

**Дальше (фаза 0)**
1. `xcodebuild -downloadComponent MetalToolchain`, `rustup update`, .NET SDK, SteamCMD, кросс-таргет для shim.
2. Пользователь логинится в SteamCMD → чистая копия в `pristine_dir` (read-only), песочница в `.local/game_bean/elden_ring/`, `config/local.toml`.
3. Проверить офлайн-запуск эталона в CrossOver из песочницы.
4. Скелет workspace: `grace-core`, `grace-app` (пустое Metal-окно `.app`), `grace-proto`, `grace-ctl` (`ping`).
