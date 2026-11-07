# Elder Grace — архитектура

## Физика задачи

Под CrossOver/GPTK Windows-игра работает так:

```
eldenring.exe (x86_64)  ─┐
Wine (x86_64)            ├─ один процесс, целиком через Rosetta 2
D3DMetal / DXVK (x86_64) ┘   → Metal
```

macOS не позволяет смешивать arm64 и x86_64 код в одном процессе. Отсюда два следствия, на которых стоит весь проект:

1. Пока исполняется оригинальный exe, **любой** наш код внутри его процесса тоже транслируется. Нативная скорость возможна только в **отдельном процессе** (IPC, общая память) или в **режиме A**, где оригинального exe нет.
2. Подстраховка трансляцией «по подсистемам» существует только в режиме C. В режиме A подстраховка бывает только целиком: играть в оригинал под CrossOver, пока наш порт не дорос.

## Режимы портирования

| Режим | Что исполняется | Плюсы | Минусы |
|---|---|---|---|
| **Эталон** | Оригинал под трансляцией | Точная игра; измерительный прибор | Это не порт |
| **C0** | Оригинал + наш shim (наблюдение) | Видно всё, что игра делает с API | Ничего не ускоряет |
| **C1** | Оригинал + подмена шейдеров | Быстрые измеримые выигрыши по GPU | Логика и API-слой под Rosetta |
| **C2** | Оригинал + рендер в отдельном arm64-процессе | Играбельно, нативный рендер | Синхронизация сцены, IPC, сложно |
| **A** | Только наш рантайм поверх данных игры | Максимальный потолок, полная нативность | Долгий паритет геймплея |
| B | Статическая рекомпиляция exe в arm64 | Точная игра | Обфускация/DRM, Win32, юридически серо. **Вне рамок проекта** |

Один и тот же код ядра (`grace-render`, `grace-capture`, `grace-ctl`, `grace-shader`) работает во всех режимах. Режим C — это способ построить инструменты и получить цифры до того, как готов режим A. Эталон остаётся с нами навсегда как источник дифференциальных тестов.

## Слои

```
                 ┌────────────────────────── эталон (Wine/GPTK, песочница) ──────────────────────────┐
                 │  оригинальный exe  ──►  shim (прокси графического API, Rust → Windows x86_64)       │
                 └──────────────┬───────────────────────────────────────┬────────────────────────────┘
                                │ loopback TCP + токен                  │ общая память (C2, позже)
                                ▼                                       ▼
 чистая установка ──►  grace-prepare ──► .local/cache/<игра>  ──►  нативный рантайм (arm64)
   (read-only)         + grace-<игра>/prepare                       grace-app ─ grace-render ─ Metal
                                                                     grace-<игра>/runtime
                                ▲
              grace-ctl / grace-mcp (управление, статистика, ожидание событий)
              grace-capture (захват и сравнение кадров) · grace-bench (маршруты, xctrace)
              grace-shader (корпус → трансляция → правка → сборка)
```

## Раскладка (целевая)

Два уровня: общая папка проекта и в ней отдельные git-репозитории (ADR 0003).

```
ProjectPort/                  общая папка, сама не под git
  elder-grace/                этот репозиторий: ядро, инструменты, shim
  grace-eldenring/            репозиторий игры: её крейты (зависят от ../elder-grace по пути) и docs/
  grace-<другая-игра>/        следующие игры — так же
  .local/                     всё локальное и игровое, общее для всех репозиториев
    game_bean/<игра>/         песочница: рабочая копия игры для эталона и shim
    cache/<игра>/  captures/  traces/  shaders/  prefixes/
```

Внутри `elder-grace`:

```
crates/                 ядро, без знаний о конкретных играх
  grace-core/           общие типы, конфиг, логирование, пути, ошибки
  grace-app/            оболочка macOS: окно, .app, ввод (GameController), меню, полноэкранный режим
  grace-render/         рендер на Metal (objc2-metal), кадровые ресурсы, MetalFX
  grace-prepare/        каркас подготовки: lock-файлы хешей, метки версий, --check, кэш
  grace-shader/         шейдерный конвейер и корпус
  grace-capture/        захват кадров, diff с эталоном
  grace-bench/          маршруты, автоматизация xctrace, отчёты
  grace-ctl/            CLI управления
  grace-mcp/            MCP-сервер поверх того же протокола
  grace-proto/          протокол ctl/shim (общие сообщения)
shim/
  graphics-proxy/       прокси DLL графического API (сначала d3d12/dxgi), cdylib под Windows x86_64
tools/                  скрипты, харнессы
xtask/                  сборочные задачи (bundle, shim)
config/                 local.example.toml (local.toml — в .gitignore)
docs/
```

Крейты появляются по мере надобности, не все сразу.

**Что есть сейчас (фаза 0):**
- `grace-core` (конфиг, путь сокета), `grace-proto` (JSON построчно: `ping`, `stats`), `grace-ctl`.
- `grace-app`: окно через **winit** 0.30 (не чистый objc2-app-kit: winit уже даёт меню с Cmd+Q, полноэкранный режим и ввод; заменим, если упрёмся), CAMetalLayer на NSView, MSL компилируется в рантайме, `--capture`, `--exit-after`, сервер управления.
- `xtask` (`bundle`, `shim`), `shim/graphics-proxy` (пустая DLL с экспортом `grace_shim_version`).
- Сокет управления по умолчанию: `$TMPDIR/elder-grace/app.sock`, переопределяется `GRACE_SOCKET`. Путь Unix-сокета ограничен ~104 байтами.

## Ключевые механизмы

### Подготовка (`grace-prepare`)
По мотивам `horizon-prepare`:
- читает только чистую установку, сверяет файлы с `prepare-lock.toml` (хеши под конкретную версию игры), незнакомую версию честно отклоняет;
- пишет в `.local/cache/<игра>/`; метку версии вывода снимает перед записью и ставит в конце, поэтому прерванный прогон не считается готовым;
- lock-файл на каталог вывода (два прогона одновременно не пишут);
- `--check` не читает данные игры, только говорит, что есть, что устарело, и какой командой это починить;
- коды выхода: 0 — готово, 1 — ошибка или пробелы, 2 — неверная команда.

### Shim
- Rust `cdylib`, собирается под `x86_64-pc-windows-*`, кладётся в песочницу, грузится через DLL override Wine.
- Режимы: проброс (по умолчанию) → логирование → дамп → подмена.
- Связь с нативной стороной через loopback TCP с токеном (Winsock в Wine работает).
- Shim исполняется под Rosetta, поэтому держим его тонким: тяжёлая работа на нативной стороне.

### Compat-shim'ы для среды эталона (`shim/compat-*`)
Маленькие прокси-DLL, которые чинят несовместимости старого Wine (GPTK = Wine 7.7) с современными Windows-программами, не трогая сам Wine.
- Кладутся рядом с exe, включаются точечно: `HKCU\Software\Wine\AppDefaults\<exe>\DllOverrides`, значение `<dll>=native,builtin`.
- Экспортируют все функции оригинала: naked-трамплины `jmp` в настоящую builtin-DLL (её грузим по полному пути из system32), чинят только нужное.
- `compat-bcrypt` → `bcrypt.dll`: переводит псевдо-хендлы алгоритмов (`BCRYPT_RNG_ALG_HANDLE` = 0x81 и т.п.) в настоящие. Проверка: `load-probe.exe bcrypt-rng` (без shim — падение, с shim — status 0). Он же — «хост» остальных исправлений, потому что его рано грузят и Steam, и `steamwebhelper`:
  - `iat.rs` — общий патчер таблиц импорта: по имени или ординалу, цель — модуль или `*` (все), плюс патч модулей при загрузке через `LdrRegisterDllNotification`;
  - `getproc_fallback.rs` — если `GetProcAddress` Wine вернул NULL, ищем функцию в compat-DLL (так Chromium получает `QueryUnbiasedInterruptTimePrecise`);
  - `winsock.rs` — `getsockname` для сокетов из `AcceptEx` (Wine 7.7 не знает их локальный адрес): обёртка `AcceptEx` через `WSAIoctl` запоминает слушающий сокет;
  - `tcp_owners.rs` — реестр TCP-соединений с PID в общей памяти префикса и достройка `GetExtendedTcpTable` (Wine на macOS отдаёт таблицу пустой).
  - Когда появится вторая игра/второй хост — вынести это в отдельный крейт `compat-core`.
- `compat-realtime` → `api-ms-win-core-realtime-l1-1-1/2.dll` в `system32`: `Query*InterruptTime*` и др. из Windows 10.
- `crash-probe.exe` — AeDebug-отладчик вместо `winedbg`: отчёт с `модуль+оффсет`, стеком и строками из памяти рядом с падением (`C:\grace-crash\`).

### Среда эталона одной командой (`cargo xtask oracle`)
`setup` (префикс, shim'ы, crash-probe, реестр, Windows-Steam; идемпотентно) → `steam --ui` (вход пользователя, один раз на префикс) → `run [игра]` (из песочницы, Metal HUD) · `status` · `kill`. Что запускать и App ID — из `config/local.toml` (`exe`, `steam_appid`). Логи — `.local/logs/`.

### Shim `d3d12.dll` (`shim/graphics-proxy`) — работает
- Прокси с ординалами как у Windows (`D3D12CreateDevice` = 101, `.def` из `build.rs`); остальные экспорты — naked-трамплины в D3DMetal.
- vtable объектов D3DMetal лежат в read-only памяти маковского фреймворка, `VirtualProtect` Wine их не меняет → объекту устройства подставляется **копия vtable** (128 слотов) с нашими слотами: `QueryInterface` (0), `CreateGraphicsPipelineState` (10), `CreateComputePipelineState` (11), `CreatePipelineLibrary` (44), `CreatePipelineState` (47, поток подобъектов).
- Шейдеры пишутся в `GRACE_SHADER_DUMP` (`oracle run` ставит `.local/shaders/<игра>/`) как `<fnv64>.<стадия>.dxbc`, сводка — `_stats.txt`.
- **Подмены на лету** (`live.rs`, `recipes.rs`): для пайплайна с подменой создаются два PSO — оригинал (его получает игра) и подменённый; таблица пар без блокировок на чтение. Командным спискам (`CreateCommandList` 12, `CreateCommandList1` 51 у ID3D12Device4) подставляется копия vtable (96 слотов; на индексе −1 — адрес исходной, поэтому вариантов vtable может быть несколько) с перехватом `QueryInterface` 0, `Reset` 10, `ClearState` 11, `SetPipelineState` 25. Номера сверены с `d3d12.h` из mingw-w64.
- **Перезагрузка с диска:** при заданном `GRACE_SHADER_OVERRIDE` для каждого PSO (устройство и библиотека) пишется рецепт — глубокая копия описания без байткода (хеши стадий; input layout, stream output, view instancing со своими строками; CachedPSO обнулён), ~1 КБ. PSO и корневой сигнатуре добавляется ссылка: адрес не переиспользуется, пару можно поставить в любой момент. `reload` перечитывает подмены и пересоздаёт подменённые PSO на устройстве игры: исходный байткод — из дампа, новый — из подмен. Старые подменённые PSO не освобождаются (могут быть в полёте на GPU).
- Управление: TCP `127.0.0.1:47322` (`GRACE_D3D12_PORT`), `grace-ctl overrides [status|on|off|toggle|reload]`, `cargo xtask overrides --reload`.

### Shim `dxgi.dll` (`shim/dxgi-proxy`) и замеры — фаза 1
- Прокси dxgi (ординалы как у GPTK, `.def` из `build.rs`). Копии vtable фабрики (`QueryInterface` 0, `CreateSwapChain` 10, `CreateSwapChainForHwnd` 15) и swapchain (`QueryInterface` 0, `Present` 8, `Present1` 22). `Present` с `DXGI_PRESENT_TEST` не считается.
- Журнал `GRACE_FRAMETIME_LOG` → CSV `frame,t_ms,dt_ms`; `oracle run --label L` пишет в `.local/frametimes/<игра>/L.csv`. `--no-shim` убирает только d3d12-прокси: время кадра меряется одинаково в обоих режимах.
- Живая статистика: TCP `127.0.0.1:47321` (`GRACE_SHIM_PORT`), протокол `grace-proto`; `grace-ctl --shim stats|watch`.
- `cargo xtask bench <метка> [<метка2>]` (`crates/grace-bench`): средний FPS, p50/p95/p99, 1% и 0.1% low, статтеры (> max(2×медиана, 25 мс)), гистограмма, сравнение B против A.
- `cargo xtask trace [--seconds N] [--label L]`: Metal System Trace (`xctrace record --attach`) запущенной игры → `.local/traces/<игра>/L.trace` → разбор: занятость GPU (объединение интервалов `metal-gpu-intervals` верхнего уровня), загрузка CPU по потокам (`ThreadCPUUsage`), вердикт GPU-bound / CPU-bound / ожидание.

### Шейдерный конвейер (`grace-shader`, `tools/grace-irconv`) — `cargo xtask shaders [игра]`
1. `grace-shader inventory` — разбор контейнеров DXBC (форматы DXBC SM4/5 и DXIL SM6), сводка по стадиям/моделям.
2. `grace-irconv` (x86_64, под Rosetta) — DXIL → `.metallib` через Apple Metal Shader Converter (`libmetalirconverter.dylib` из D3DMetal в GPTK, только x86_64; используется локально). Вывод: `.local/shaders/<игра>-metal/`.
3. `grace-shader metallib` — нативная (arm64) загрузка каждого `metallib` в `MTLDevice` и создание функций.
4. DXBC SM4/5 → SPIR-V (`vkd3d-compiler` 1.19, собран из исходников в `.local/tools/bin`) → MSL (SPIRV-Cross `--msl-version 30000`) → `xcrun metal` + `metallib`. Промежуточные `.spv`/`.metal`/`.air` — в `.local/shaders/<игра>-msl/`.
5. Сборка vkd3d (один раз): исходники с dl.winehq.org → `./configure --prefix=<ProjectPort>/.local/tools --disable-tests --disable-doxygen-doc BISON=/opt/homebrew/opt/bison/bin/bison LEX=/opt/homebrew/opt/flex/bin/flex CPPFLAGS=-I/opt/homebrew/include LDFLAGS=-L/opt/homebrew/lib` → полный `make` (целевой `make vkd3d-compiler` пропускает генерацию заголовков) → `make install`. Нужны из Homebrew: `bison`, `flex`, `spirv-headers`, `vulkan-headers`, `molten-vk` (только для `configure`), `spirv-cross`.
6. Пока это **проверка компилируемости**: привязки ресурсов (корневая сигнатура D3D12 → аргументы Metal) не сопоставлены, шейдеры ещё не рисуют. Это следующий шаг к C1/C2.
- Корпус: каждый шейдер по хешу, метаданные (стадия, проход, стоимость из захватов).
- Трансляция в читаемый вид и обратно — инструменты подбираем на фазе 3 (кандидаты в `REFERENCES.md`).
- Для режима A — сборка MSL в `metallib` и Metal binary archives, чтобы не было статтеров компиляции.

### Управление (`grace-ctl` / `grace-mcp`)
- Один протокол JSON поверх Unix-сокета (рантайм) или loopback TCP (shim: dxgi — 47321, статистика; d3d12 — 47322, подмены).
- Команды: `ping`, `stats`, `camera set/get`, `feature on/off`, `capture`, `wait-for <событие>`, `events` (без опроса).
- MCP-сервер даёт агенту те же команды напрямую.

### Сравнение с эталоном
- Камеру эталона читаем из констант (shim), ставим ту же в наш рантайм, снимаем оба кадра, `grace-capture diff` выдаёт метрику и карту различий.
- Перф: один маршрут, один пресет, одно разрешение — цифры в `../grace-<игра>/docs/BASELINE.md` и `PROGRESS.md`.

## Целевое железо

- Разработка: M5 Pro, 20 GPU-ядер, 48 ГБ.
- Минимум: M4 (конфиг друга уточнить), закладываемся на 16 ГБ unified memory.
- Фичи Metal — по проверке возможностей устройства. Бюджеты памяти и стриминга — от минимальной конфигурации.
