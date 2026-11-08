//! Viewer window: CAMetalLayer, mouse orbit camera, wheel to zoom,
//! WASD/QE to fly (Shift for faster), Esc/Cmd+Q to quit.
//! An optional camera driver (`run_with`) can steer the camera every frame (a live game).

use anyhow::{Context, Result, bail};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::NSView;
use objc2_core_foundation::CGSize;
use objc2_metal::MTLCommandBuffer;
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowId};

use crate::gpu::{COLOR_FORMAT, Texture};
use crate::{Camera, Gpu, GpuScene};

struct Live {
    // The layer hangs on the window's view, so it must die before the window.
    layer: Retained<CAMetalLayer>,
    depth: Texture,
    window: Window,
}

/// Called once per frame before drawing; may move the camera and return a new window title.
pub type CameraDriver = Box<dyn FnMut(&mut Camera) -> Option<String>>;

struct Viewer {
    title: String,
    driver: Option<CameraDriver>,
    gpu: Gpu,
    scene: GpuScene,
    camera: Camera,
    live: Option<Live>,
    dragging: bool,
    last: Option<(f64, f64)>,
    modifiers: ModifiersState,
    /// Fly keys held down: W S A D Q E.
    keys: std::collections::HashSet<String>,
    last_frame: Option<std::time::Instant>,
    error: Option<anyhow::Error>,
}

impl Viewer {
    fn init(&mut self, el: &ActiveEventLoop) -> Result<()> {
        let window = el.create_window(Window::default_attributes().with_title(&self.title).with_inner_size(LogicalSize::new(1280.0, 800.0)))?;
        let RawWindowHandle::AppKit(h) = window.window_handle()?.as_raw() else { bail!("expected an AppKit handle") };
        // SAFETY: ns_view lives as long as the window; the window outlives the layer (field order of Live).
        let view: &NSView = unsafe { h.ns_view.cast().as_ref() };
        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&self.gpu.device));
        layer.setPixelFormat(COLOR_FORMAT);
        layer.setFramebufferOnly(true);
        view.setWantsLayer(true);
        view.setLayer(Some(&layer));
        let size = window.inner_size();
        layer.setContentsScale(window.scale_factor());
        layer.setDrawableSize(CGSize::new(f64::from(size.width.max(1)), f64::from(size.height.max(1))));
        let depth = self.gpu.depth_texture(size.width as usize, size.height as usize)?;
        window.request_redraw();
        self.live = Some(Live { layer, depth, window });
        Ok(())
    }

    /// WASD: forward/sideways, Q/E: down/up, Shift: faster.
    fn fly(&mut self) {
        let now = std::time::Instant::now();
        let dt = self.last_frame.map_or(0.0, |t| (now - t).as_secs_f32()).min(0.1);
        self.last_frame = Some(now);
        let key = |k: &str| if self.keys.contains(k) { 1.0 } else { 0.0 };
        let speed = (self.camera.distance * 1.5).max(2.0) * if self.modifiers.shift_key() { 4.0 } else { 1.0 } * dt;
        let (f, r, u) = (key("w") - key("s"), key("d") - key("a"), key("e") - key("q"));
        if f != 0.0 || r != 0.0 || u != 0.0 {
            self.camera.fly(f * speed, r * speed, u * speed);
        }
    }

    fn draw(&mut self) {
        self.fly();
        if let Some(driver) = self.driver.as_mut()
            && let Some(title) = driver(&mut self.camera)
            && let Some(live) = &self.live
        {
            live.window.set_title(&title);
        }
        let Some(live) = &self.live else { return };
        autoreleasepool(|_| {
            let Some(drawable) = live.layer.nextDrawable() else { return };
            if let Some(cmd) = self.gpu.encode(&drawable.texture(), &live.depth, &self.scene, &self.camera) {
                cmd.presentDrawable(ProtocolObject::from_ref(&*drawable));
                cmd.commit();
            }
        });
    }
}

impl ApplicationHandler for Viewer {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.live.is_none()
            && let Err(e) = self.init(el)
        {
            self.error = Some(e);
            el.exit();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) if pressed => el.exit(),
                    Key::Character(c) if c == "q" && self.modifiers.super_key() && pressed => el.exit(),
                    Key::Character(c) => {
                        let c = c.to_lowercase();
                        if pressed {
                            self.keys.insert(c);
                        } else {
                            self.keys.remove(&c);
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(live) = &mut self.live {
                    live.layer.setDrawableSize(CGSize::new(f64::from(size.width.max(1)), f64::from(size.height.max(1))));
                    if let Ok(d) = self.gpu.depth_texture(size.width as usize, size.height as usize) {
                        live.depth = d;
                    }
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                self.dragging = state == ElementState::Pressed;
                self.last = None;
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.dragging {
                    if let Some((x, y)) = self.last {
                        self.camera.yaw -= ((position.x - x) * 0.005) as f32;
                        self.camera.pitch = (self.camera.pitch + ((position.y - y) * 0.005) as f32).clamp(-1.5, 1.5);
                    }
                    self.last = Some((position.x, position.y));
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 0.1,
                    MouseScrollDelta::PixelDelta(p) => (p.y * 0.002) as f32,
                };
                self.camera.distance = (self.camera.distance * (1.0 - dy)).max(self.camera.near * 2.0);
            }
            WindowEvent::RedrawRequested => {
                self.draw();
                if let Some(live) = &self.live {
                    live.window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

/// Opens a window and runs the loop until it is closed.
pub fn run(title: &str, gpu: Gpu, scene: GpuScene, camera: Camera) -> Result<()> {
    run_with(title, gpu, scene, camera, None)
}

/// Like `run`, with a camera driver called every frame.
pub fn run_with(title: &str, gpu: Gpu, scene: GpuScene, camera: Camera, driver: Option<CameraDriver>) -> Result<()> {
    let el = EventLoop::new().context("event loop")?;
    el.set_control_flow(ControlFlow::Poll);
    let mut v = Viewer { title: title.to_owned(), driver, gpu, scene, camera, live: None, dragging: false, last: None, modifiers: ModifiersState::empty(), keys: Default::default(), last_frame: None, error: None };
    el.run_app(&mut v)?;
    v.error.map_or(Ok(()), Err)
}
