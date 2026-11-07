# Сторонний код и инструменты

Сюда записываем всё, что берём **кодом** (зависимости, форки, перенесённые фрагменты), и всё, что используем как внешний инструмент в разработке. Перед добавлением лицензию проверяем по самому репозиторию.

| Проект | Версия / коммит | Лицензия (сверена) | Как используем | Где в репо |
|---|---|---|---|---|
| winit | 0.30.13 | Apache-2.0 (cargo metadata, 2026-11-07) | Окно, события, ввод; тянет objc2 0.5 параллельно с нашим 0.6 | `grace-app` |
| objc2, objc2-foundation | 0.6.5 / 0.3.2 | MIT | Мост в Objective-C | `grace-app` |
| objc2-metal, objc2-quartz-core, objc2-app-kit, objc2-core-foundation | 0.3.2 | Zlib OR Apache-2.0 OR MIT | Metal, CAMetalLayer, NSView | `grace-app` |
| png | 0.18.1 | MIT OR Apache-2.0 | Запись кадров `--capture` | `grace-app` |
| serde, serde_json, toml | 1.x | MIT OR Apache-2.0 | Протокол, конфиг | `grace-core`, `grace-proto` |
| clap, anyhow, thiserror | 4.x / 1.x / 2.x | MIT OR Apache-2.0 | CLI, ошибки | везде |

Транзитивные зависимости: смотреть `cargo metadata`; перед первым распространением сборки прогнать проверку лицензий (`cargo deny` или аналог).

Инструменты конвейера шейдеров (только локально, не распространяются):
- Apple Metal Shader Converter — `libmetalirconverter.dylib` из D3DMetal внутри GPTK; вызывается `tools/grace-irconv` через `dlopen`, в репозитории его нет.
- SPIRV-Cross 1.4.363.0 (Homebrew, Apache-2.0) — ветка DXBC → SPIR-V → MSL в `cargo xtask shaders`, вызывается как внешняя программа.
- vkd3d 1.19 (WineHQ, LGPL-2.1) — исходники в `.local/tools/src` (sha256 `034613605baab8ba84674f8d272cf22b5e86bc6bc03fc5728ef9bce07308baa6`), собран локально, `vkd3d-compiler` вызывается как внешняя программа (не линкуется в наш код).
- Homebrew для сборки vkd3d: bison, flex, spirv-headers, vulkan-headers, molten-vk (Apache-2.0).

Инструменты разработки без распространения (только локально): CrossOver, Apple Game Porting Toolkit (D3DMetal), Apple Metal Shader Converter, Xcode/Instruments. В репозиторий и сборки не попадают.
