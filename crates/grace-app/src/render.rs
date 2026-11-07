//! Минимальный рендер на Metal: один пайплайн, вывод в окно (CAMetalLayer)
//! или в офскрин-текстуру для `--capture`.
//! Шейдеры пока компилируются в рантайме из исходника: офлайн-сборке в metallib
//! нужен Metal Toolchain, а он появится позже.

use std::path::Path;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::NSView;
use objc2_core_foundation::CGSize;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice,
    MTLLibrary, MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRegion, MTLRenderCommandEncoder,
    MTLRenderPassDescriptor, MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLSize, MTLStorageMode,
    MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};

const PIXEL_FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm_sRGB;

/// Устройство, очередь и пайплайны — всё, что не зависит от того, куда рисуем.
pub struct Gpu {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
}

impl Gpu {
    pub fn new() -> Result<Self> {
        let device = MTLCreateSystemDefaultDevice().context("нет Metal-устройства")?;
        let queue = device.newCommandQueue().context("не создаётся очередь команд")?;

        let source = NSString::from_str(include_str!("../shaders/triangle.metal"));
        let library = device
            .newLibraryWithSource_options_error(&source, None)
            .map_err(|e| anyhow!("ошибка компиляции MSL: {}", e.localizedDescription()))?;
        let function = |name: &str| {
            library
                .newFunctionWithName(&NSString::from_str(name))
                .with_context(|| format!("нет функции {name}"))
        };

        let desc = MTLRenderPipelineDescriptor::new();
        let (vs, fs) = (function("grace_vs")?, function("grace_fs")?);
        desc.setVertexFunction(Some(&vs));
        desc.setFragmentFunction(Some(&fs));
        // SAFETY: индекс 0 всегда валиден для colorAttachments.
        unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) }.setPixelFormat(PIXEL_FORMAT);
        let pipeline = device
            .newRenderPipelineStateWithDescriptor_error(&desc)
            .map_err(|e| anyhow!("не создаётся пайплайн: {}", e.localizedDescription()))?;

        Ok(Self { device, queue, pipeline })
    }

    pub fn device_name(&self) -> String {
        self.device.name().to_string()
    }

    /// Записывает кадр в `target`. Возвращает незакоммиченный командный буфер.
    fn encode(
        &self,
        target: &ProtocolObject<dyn MTLTexture>,
        time: f32,
    ) -> Option<Retained<ProtocolObject<dyn MTLCommandBuffer>>> {
        let pass = MTLRenderPassDescriptor::new();
        // SAFETY: индекс 0 всегда валиден для colorAttachments.
        let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        color.setTexture(Some(target));
        color.setLoadAction(MTLLoadAction::Clear);
        color.setClearColor(MTLClearColor { red: 0.04, green: 0.04, blue: 0.06, alpha: 1.0 });
        color.setStoreAction(MTLStoreAction::Store);

        let cmd = self.queue.commandBuffer()?;
        let enc = cmd.renderCommandEncoderWithDescriptor(&pass)?;
        enc.setRenderPipelineState(&self.pipeline);
        // SAFETY: указатель на f32 на стеке живёт до конца вызова, Metal копирует байты сразу.
        unsafe {
            enc.setFragmentBytes_length_atIndex(NonNull::from(&time).cast(), size_of::<f32>(), 0);
            enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3);
        }
        enc.endEncoding();
        Some(cmd)
    }

    /// Рисует один кадр в офскрин-текстуру и сохраняет PNG.
    pub fn capture_png(&self, width: u32, height: u32, time: f32, out: &Path) -> Result<()> {
        let (w, h) = (width as usize, height as usize);
        // SAFETY: формат и размеры валидны, mipmap не нужен.
        let td = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(PIXEL_FORMAT, w, h, false)
        };
        td.setUsage(MTLTextureUsage::RenderTarget);
        // Unified memory: Shared-текстуру CPU читает без blit-копии.
        td.setStorageMode(MTLStorageMode::Shared);
        let texture = self.device.newTextureWithDescriptor(&td).context("не создаётся текстура захвата")?;

        let cmd = self.encode(&texture, time).context("не записывается кадр")?;
        cmd.commit();
        cmd.waitUntilCompleted();

        let mut pixels = vec![0u8; w * h * 4];
        let region = MTLRegion { origin: MTLOrigin { x: 0, y: 0, z: 0 }, size: MTLSize { width: w, height: h, depth: 1 } };
        // SAFETY: буфер ровно w*h*4 байт, bytesPerRow = w*4, GPU-работа завершена.
        unsafe {
            texture.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                NonNull::new(pixels.as_mut_ptr()).context("пустой буфер")?.cast(),
                w * 4,
                region,
                0,
            );
        }
        for px in pixels.as_chunks_mut::<4>().0 {
            px.swap(0, 2); // BGRA → RGBA
        }

        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::io::BufWriter::new(std::fs::File::create(out)?);
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        encoder.write_header()?.write_image_data(&pixels)?;
        Ok(())
    }
}

/// Вывод в окно.
pub struct Renderer {
    gpu: Gpu,
    layer: Retained<CAMetalLayer>,
    started: Instant,
    drawable_size: (u32, u32),
}

impl Renderer {
    pub fn new(gpu: Gpu, view: &NSView, width: u32, height: u32, scale: f64) -> Self {
        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&gpu.device));
        layer.setPixelFormat(PIXEL_FORMAT);
        layer.setFramebufferOnly(true);
        view.setWantsLayer(true);
        view.setLayer(Some(&layer));

        let mut renderer = Self { gpu, layer, started: Instant::now(), drawable_size: (0, 0) };
        renderer.resize(width, height, scale);
        renderer
    }

    pub fn device_name(&self) -> String {
        self.gpu.device_name()
    }

    pub fn drawable_size(&self) -> (u32, u32) {
        self.drawable_size
    }

    pub fn resize(&mut self, width: u32, height: u32, scale: f64) {
        let (w, h) = (width.max(1), height.max(1));
        self.layer.setContentsScale(scale);
        self.layer.setDrawableSize(CGSize::new(f64::from(w), f64::from(h)));
        self.drawable_size = (w, h);
    }

    /// Рисует кадр. Возвращает время CPU-работы над кадром — без ожидания
    /// drawable, которое на самом деле ожидание vsync.
    pub fn draw(&mut self) -> Option<Duration> {
        autoreleasepool(|_| {
            let drawable = self.layer.nextDrawable()?;
            let t0 = Instant::now();
            let cmd = self.gpu.encode(&drawable.texture(), self.started.elapsed().as_secs_f32())?;
            cmd.presentDrawable(ProtocolObject::from_ref(&*drawable));
            cmd.commit();
            Some(t0.elapsed())
        })
    }
}
