//! Grace.app — оболочка macOS и нативный рантайм Elder Grace.
//!
//! Фаза 0: окно, Metal, тестовый треугольник, сервер управления.
//! Флаги:
//! - `--exit-after <сек>` — выйти сам (для автоматических проверок);
//! - `--capture <out.png> [--size WxH]` — отрисовать один кадр в файл без окна.

mod control;
mod render;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use grace_proto::Stats;
use objc2_app_kit::NSView;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Fullscreen, Window, WindowId};

use control::{ControlServer, SharedStats};
use render::{Gpu, Renderer};

struct Running {
    // Порядок полей важен: рендер держит слой на view окна и должен умереть раньше окна.
    renderer: Renderer,
    window: Window,
    frames: u64,
    window_start: Instant,
    window_frames: u32,
    window_cpu: Duration,
}

struct App {
    running: Option<Running>,
    stats: SharedStats,
    modifiers: ModifiersState,
    deadline: Option<Instant>,
    error: Option<anyhow::Error>,
}

impl App {
    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let attrs = Window::default_attributes()
            .with_title("Elder Grace")
            .with_inner_size(LogicalSize::new(1280.0, 800.0));
        let window = event_loop.create_window(attrs).context("не создаётся окно")?;

        let handle = window.window_handle().context("нет handle окна")?.as_raw();
        let RawWindowHandle::AppKit(appkit) = handle else {
            bail!("ожидался AppKit-handle, получен {handle:?}");
        };
        // SAFETY: winit гарантирует, что ns_view — живой NSView, пока живо окно,
        // а окно живёт дольше рендера (см. порядок полей Running).
        let view: &NSView = unsafe { appkit.ns_view.cast().as_ref() };

        let size = window.inner_size();
        let renderer = Renderer::new(Gpu::new()?, view, size.width, size.height, window.scale_factor());
        eprintln!("[grace] Metal: {}", renderer.device_name());
        if let Ok(mut s) = self.stats.lock() {
            s.device = renderer.device_name();
        }

        window.request_redraw();
        self.running = Some(Running {
            renderer,
            window,
            frames: 0,
            window_start: Instant::now(),
            window_frames: 0,
            window_cpu: Duration::ZERO,
        });
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        if let Err(e) = self.init(event_loop) {
            self.error = Some(e);
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(run) = self.running.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => run.renderer.resize(size.width, size.height, run.window.scale_factor()),
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = run.window.inner_size();
                run.renderer.resize(size.width, size.height, run.window.scale_factor());
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput {
                event: KeyEvent { logical_key: Key::Character(ref c), state: ElementState::Pressed, repeat: false, .. },
                ..
            } if c.eq_ignore_ascii_case("f") && self.modifiers.super_key() && self.modifiers.control_key() => {
                // Ctrl+Cmd+F — стандартное для macOS переключение полноэкранного режима.
                let next = match run.window.fullscreen() {
                    Some(_) => None,
                    None => Some(Fullscreen::Borderless(None)),
                };
                run.window.set_fullscreen(next);
            }
            WindowEvent::RedrawRequested => {
                if let Some(cpu) = run.renderer.draw() {
                    run.frames += 1;
                    run.window_frames += 1;
                    run.window_cpu += cpu;
                }

                let elapsed = run.window_start.elapsed();
                if elapsed >= Duration::from_secs(1) && run.window_frames > 0 {
                    if let Ok(mut s) = self.stats.lock() {
                        let (w, h) = run.renderer.drawable_size();
                        s.frames = run.frames;
                        s.fps = f64::from(run.window_frames) / elapsed.as_secs_f64();
                        s.frame_ms = run.window_cpu.as_secs_f64() * 1000.0 / f64::from(run.window_frames);
                        s.drawable_width = w;
                        s.drawable_height = h;
                    }
                    run.window_start = Instant::now();
                    run.window_frames = 0;
                    run.window_cpu = Duration::ZERO;
                }
                if self.deadline.is_some_and(|d| Instant::now() >= d) {
                    event_loop.exit();
                    return;
                }
                run.window.request_redraw();
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct Args {
    exit_after: Option<Duration>,
    capture: Option<std::path::PathBuf>,
    size: (u32, u32),
}

fn parse_args() -> Result<Args> {
    let mut out = Args { size: (1920, 1080), ..Args::default() };
    let mut args = std::env::args().skip(1);
    // Finder может передать -psn_…; всё незнакомое игнорируем.
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exit-after" => {
                let secs: f64 = args.next().context("--exit-after требует число секунд")?.parse()?;
                out.exit_after = Some(Duration::from_secs_f64(secs));
            }
            "--capture" => out.capture = Some(args.next().context("--capture требует путь к PNG")?.into()),
            "--size" => {
                let v = args.next().context("--size требует WxH")?;
                let (w, h) = v.split_once('x').context("--size в формате WxH")?;
                out.size = (w.parse()?, h.parse()?);
            }
            _ => {}
        }
    }
    Ok(out)
}

fn main() -> Result<()> {
    let args = parse_args()?;
    if let Some(out) = &args.capture {
        let gpu = Gpu::new()?;
        gpu.capture_png(args.size.0, args.size.1, 0.0, out)?;
        eprintln!("[grace] кадр {}×{} ({}) → {}", args.size.0, args.size.1, gpu.device_name(), out.display());
        return Ok(());
    }
    let exit_after = args.exit_after;
    let stats: SharedStats = Arc::new(Mutex::new(Stats {
        frames: 0,
        frame_ms: 0.0,
        fps: 0.0,
        drawable_width: 0,
        drawable_height: 0,
        device: String::new(),
    }));
    let _control = ControlServer::start(&grace_core::paths::control_socket(), Arc::clone(&stats))?;

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        running: None,
        stats: Arc::clone(&stats),
        modifiers: ModifiersState::empty(),
        deadline: exit_after.map(|d| Instant::now() + d),
        error: None,
    };
    event_loop.run_app(&mut app)?;
    if let Some(e) = app.error {
        return Err(e);
    }
    if let Ok(s) = stats.lock() {
        eprintln!("[grace] выход: кадров {}, последние {:.1} FPS", s.frames, s.fps);
    }
    Ok(())
}
