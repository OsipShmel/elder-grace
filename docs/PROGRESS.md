# Журнал прогресса

Новые записи — сверху. Каждая запись: что сделано, как проверено, цифры, что дальше, блокеры.

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
