//! Metal: устройство, пайплайн мешей с глубиной, загрузка сцены, кадр в текстуру и в PNG.

use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;

use anyhow::{Context, Result, anyhow};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBuffer, MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLCompareFunction,
    MTLCreateSystemDefaultDevice, MTLDepthStencilDescriptor, MTLDepthStencilState, MTLDevice, MTLIndexType,
    MTLLibrary, MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRegion, MTLRenderCommandEncoder,
    MTLRenderPassDescriptor, MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLResourceOptions, MTLSize,
    MTLSamplerAddressMode, MTLSamplerDescriptor, MTLSamplerMinMagFilter, MTLSamplerMipFilter, MTLSamplerState,
    MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};

use crate::dds::{Format, TextureData};
use crate::{Camera, Scene};

pub const COLOR_FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm_sRGB;
pub const DEPTH_FORMAT: MTLPixelFormat = MTLPixelFormat::Depth32Float;

type Device = ProtocolObject<dyn MTLDevice>;
type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;
pub type Texture = Retained<ProtocolObject<dyn MTLTexture>>;

#[repr(C)]
struct Uniforms {
    view_proj: [f32; 16],
    light: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
}

struct GpuMesh {
    positions: Buffer,
    normals: Buffer,
    uvs: Buffer,
    indices: Buffer,
    index_count: usize,
    color: [f32; 4],
    texture: Option<usize>,
}

/// Сцена в памяти GPU (unified memory: буферы Shared, без копий через blit).
pub struct GpuScene {
    meshes: Vec<GpuMesh>,
    textures: Vec<Texture>,
}

pub struct Gpu {
    pub(crate) device: Retained<Device>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    depth_state: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
    sampler: Retained<ProtocolObject<dyn MTLSamplerState>>,
    /// 1×1 белая — привязывается, когда у меша нет текстуры.
    white: Texture,
}

fn pixel_format(f: Format) -> MTLPixelFormat {
    match f {
        Format::Bc1 => MTLPixelFormat::BC1_RGBA,
        Format::Bc1Srgb => MTLPixelFormat::BC1_RGBA_sRGB,
        Format::Bc2 => MTLPixelFormat::BC2_RGBA,
        Format::Bc2Srgb => MTLPixelFormat::BC2_RGBA_sRGB,
        Format::Bc3 => MTLPixelFormat::BC3_RGBA,
        Format::Bc3Srgb => MTLPixelFormat::BC3_RGBA_sRGB,
        Format::Bc4 => MTLPixelFormat::BC4_RUnorm,
        Format::Bc5 => MTLPixelFormat::BC5_RGUnorm,
        Format::Bc6hUf => MTLPixelFormat::BC6H_RGBUfloat,
        Format::Bc7 => MTLPixelFormat::BC7_RGBAUnorm,
        Format::Bc7Srgb => MTLPixelFormat::BC7_RGBAUnorm_sRGB,
        Format::Rgba8 => MTLPixelFormat::RGBA8Unorm,
        Format::Rgba8Srgb => MTLPixelFormat::RGBA8Unorm_sRGB,
        Format::Bgra8 => MTLPixelFormat::BGRA8Unorm,
        Format::Bgra8Srgb => MTLPixelFormat::BGRA8Unorm_sRGB,
    }
}

/// Текстура со всеми уровнями мипов как есть (BCn без перекодирования).
fn texture(device: &Device, t: &TextureData) -> Result<Texture> {
    // SAFETY: формат и размеры валидны.
    let td = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(pixel_format(t.format), t.width, t.height, false) };
    // SAFETY: число уровней не больше, чем допускает размер (уровни взяты из DDS, ≥ 1).
    unsafe { td.setMipmapLevelCount(t.mips.len()) };
    td.setUsage(MTLTextureUsage::ShaderRead);
    let tex = device.newTextureWithDescriptor(&td).context("не создаётся текстура")?;
    let (mut w, mut h) = (t.width, t.height);
    for (level, bytes) in t.mips.iter().enumerate() {
        let (row, _) = t.format.level_size(w, h);
        let region = MTLRegion { origin: MTLOrigin { x: 0, y: 0, z: 0 }, size: MTLSize { width: w, height: h, depth: 1 } };
        // SAFETY: bytes — ровно один уровень этого размера (проверено при разборе DDS).
        unsafe { tex.replaceRegion_mipmapLevel_withBytes_bytesPerRow(region, level, NonNull::new(bytes.as_ptr() as *mut c_void).context("пустой уровень")?, row) };
        (w, h) = ((w / 2).max(1), (h / 2).max(1));
    }
    Ok(tex)
}

fn buffer<T: Copy>(device: &Device, data: &[T]) -> Result<Buffer> {
    let len = std::mem::size_of_val(data).max(16);
    let ptr = NonNull::new(data.as_ptr() as *mut c_void).context("пустой буфер")?;
    // SAFETY: data живёт до конца вызова, Metal копирует байты в новый буфер.
    unsafe { device.newBufferWithBytes_length_options(ptr, len, MTLResourceOptions::StorageModeShared) }.context("не создаётся буфер")
}

/// Цвет меша по номеру — чтобы части модели различались, пока нет текстур.
fn palette(i: usize) -> [f32; 4] {
    const P: [[f32; 3]; 6] = [[0.62, 0.58, 0.52], [0.45, 0.52, 0.42], [0.55, 0.47, 0.40], [0.50, 0.50, 0.56], [0.60, 0.54, 0.44], [0.42, 0.46, 0.50]];
    let c = P[i % P.len()];
    [c[0], c[1], c[2], 1.0]
}

impl Gpu {
    pub fn new() -> Result<Self> {
        let device = MTLCreateSystemDefaultDevice().context("нет Metal-устройства")?;
        let queue = device.newCommandQueue().context("не создаётся очередь")?;
        let library = device
            .newLibraryWithSource_options_error(&NSString::from_str(include_str!("../shaders/mesh.metal")), None)
            .map_err(|e| anyhow!("ошибка MSL: {}", e.localizedDescription()))?;
        let function = |name: &str| library.newFunctionWithName(&NSString::from_str(name)).with_context(|| format!("нет функции {name}"));
        let desc = MTLRenderPipelineDescriptor::new();
        let (vs, fs) = (function("grace_mesh_vs")?, function("grace_mesh_fs")?);
        desc.setVertexFunction(Some(&vs));
        desc.setFragmentFunction(Some(&fs));
        // SAFETY: индекс 0 всегда валиден для colorAttachments.
        unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) }.setPixelFormat(COLOR_FORMAT);
        desc.setDepthAttachmentPixelFormat(DEPTH_FORMAT);
        let pipeline = device.newRenderPipelineStateWithDescriptor_error(&desc).map_err(|e| anyhow!("пайплайн: {}", e.localizedDescription()))?;
        let ds = MTLDepthStencilDescriptor::new();
        ds.setDepthCompareFunction(MTLCompareFunction::Less);
        ds.setDepthWriteEnabled(true);
        let depth_state = device.newDepthStencilStateWithDescriptor(&ds).context("не создаётся состояние глубины")?;
        let sd = MTLSamplerDescriptor::new();
        sd.setMinFilter(MTLSamplerMinMagFilter::Linear);
        sd.setMagFilter(MTLSamplerMinMagFilter::Linear);
        sd.setMipFilter(MTLSamplerMipFilter::Linear);
        sd.setSAddressMode(MTLSamplerAddressMode::Repeat);
        sd.setTAddressMode(MTLSamplerAddressMode::Repeat);
        sd.setMaxAnisotropy(8);
        let sampler = device.newSamplerStateWithDescriptor(&sd).context("не создаётся сэмплер")?;
        let white = texture(&device, &TextureData { format: Format::Rgba8, width: 1, height: 1, mips: vec![vec![255; 4]] })?;
        Ok(Self { device, queue, pipeline, depth_state, sampler, white })
    }

    pub fn device_name(&self) -> String {
        self.device.name().to_string()
    }

    pub fn upload(&self, scene: &Scene) -> Result<GpuScene> {
        let meshes = scene
            .meshes
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.indices.is_empty() && !m.positions.is_empty())
            .map(|(i, m)| {
                let uvs = if m.uvs.len() == m.positions.len() { m.uvs.clone() } else { vec![[0.0; 2]; m.positions.len()] };
                Ok(GpuMesh {
                    positions: buffer(&self.device, &m.positions)?,
                    normals: buffer(&self.device, &m.normals)?,
                    uvs: buffer(&self.device, &uvs)?,
                    indices: buffer(&self.device, &m.indices)?,
                    index_count: m.indices.len(),
                    color: m.color.unwrap_or_else(|| palette(i)),
                    texture: m.texture.filter(|&t| t < scene.textures.len() && m.uvs.len() == m.positions.len()),
                })
            })
            .collect::<Result<_>>()?;
        let textures = scene.textures.iter().map(|t| texture(&self.device, t)).collect::<Result<_>>()?;
        Ok(GpuScene { meshes, textures })
    }

    /// Текстура глубины под размер цели; Memoryless — живёт только в тайловой памяти GPU.
    pub fn depth_texture(&self, w: usize, h: usize) -> Result<Texture> {
        // SAFETY: формат и размеры валидны.
        let td = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(DEPTH_FORMAT, w.max(1), h.max(1), false) };
        td.setUsage(MTLTextureUsage::RenderTarget);
        td.setStorageMode(MTLStorageMode::Memoryless);
        self.device.newTextureWithDescriptor(&td).context("не создаётся глубина")
    }

    pub(crate) fn encode(
        &self,
        target: &ProtocolObject<dyn MTLTexture>,
        depth: &ProtocolObject<dyn MTLTexture>,
        scene: &GpuScene,
        camera: &Camera,
    ) -> Option<Retained<ProtocolObject<dyn MTLCommandBuffer>>> {
        let pass = MTLRenderPassDescriptor::new();
        // SAFETY: индекс 0 всегда валиден для colorAttachments.
        let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        color.setTexture(Some(target));
        color.setLoadAction(MTLLoadAction::Clear);
        color.setClearColor(MTLClearColor { red: 0.32, green: 0.36, blue: 0.42, alpha: 1.0 });
        color.setStoreAction(MTLStoreAction::Store);
        let d = pass.depthAttachment();
        d.setTexture(Some(depth));
        d.setLoadAction(MTLLoadAction::Clear);
        d.setClearDepth(1.0);
        d.setStoreAction(MTLStoreAction::DontCare);

        let aspect = target.width() as f32 / target.height().max(1) as f32;
        let view_proj = camera.view_proj(aspect);
        let light = crate::math::normalize([0.4, 0.8, 0.3]);

        let cmd = self.queue.commandBuffer()?;
        let enc = cmd.renderCommandEncoderWithDescriptor(&pass)?;
        enc.setRenderPipelineState(&self.pipeline);
        enc.setDepthStencilState(Some(&self.depth_state));
        // SAFETY: сэмплер жив, пока жив Gpu.
        unsafe { enc.setFragmentSamplerState_atIndex(Some(&self.sampler), 0) };
        for m in &scene.meshes {
            let tex = m.texture.and_then(|t| scene.textures.get(t));
            let has = if tex.is_some() { 1.0 } else { 0.0 };
            let u = Uniforms { view_proj, light: [light[0], light[1], light[2], 0.0], color: m.color, params: [has, 0.0, 0.0, 0.0] };
            // SAFETY: буферы живы, пока жива сцена (до конца кадра — commit/wait снаружи);
            // uniforms копируются Metal сразу; индексы проверены парсером (< числа вершин).
            unsafe {
                enc.setVertexBuffer_offset_atIndex(Some(&m.positions), 0, 0);
                enc.setVertexBuffer_offset_atIndex(Some(&m.normals), 0, 1);
                enc.setVertexBuffer_offset_atIndex(Some(&m.uvs), 0, 2);
                enc.setVertexBytes_length_atIndex(NonNull::from(&u).cast(), size_of::<Uniforms>(), 3);
                enc.setFragmentTexture_atIndex(Some(tex.unwrap_or(&self.white)), 0);
                enc.setFragmentBytes_length_atIndex(NonNull::from(&u).cast(), size_of::<Uniforms>(), 0);
                enc.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset(
                    MTLPrimitiveType::Triangle,
                    m.index_count,
                    MTLIndexType::UInt32,
                    &m.indices,
                    0,
                );
            }
        }
        enc.endEncoding();
        Some(cmd)
    }

    /// Один кадр без окна → PNG.
    pub fn capture_png(&self, scene: &GpuScene, camera: &Camera, width: u32, height: u32, out: &Path) -> Result<()> {
        let (w, h) = (width as usize, height as usize);
        // SAFETY: формат и размеры валидны.
        let td = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(COLOR_FORMAT, w, h, false) };
        td.setUsage(MTLTextureUsage::RenderTarget);
        td.setStorageMode(MTLStorageMode::Shared);
        let texture = self.device.newTextureWithDescriptor(&td).context("текстура захвата")?;
        let depth = self.depth_texture(w, h)?;
        let cmd = self.encode(&texture, &depth, scene, camera).context("кадр не записан")?;
        cmd.commit();
        cmd.waitUntilCompleted();

        let mut pixels = vec![0u8; w * h * 4];
        let region = MTLRegion { origin: MTLOrigin { x: 0, y: 0, z: 0 }, size: MTLSize { width: w, height: h, depth: 1 } };
        // SAFETY: буфер w*h*4, bytesPerRow w*4, GPU-работа завершена.
        unsafe {
            texture.getBytes_bytesPerRow_fromRegion_mipmapLevel(NonNull::new(pixels.as_mut_ptr()).context("буфер")?.cast(), w * 4, region, 0);
        }
        for px in pixels.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(out)?), width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        enc.write_header()?.write_image_data(&pixels)?;
        Ok(())
    }
}
