//! DDS → данные текстуры: BCn и 8-битные форматы, все мипы (без перекодирования —
//! Apple Silicon читает BCn напрямую).

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Bc1,
    Bc1Srgb,
    Bc2,
    Bc2Srgb,
    Bc3,
    Bc3Srgb,
    Bc4,
    Bc5,
    Bc6hUf,
    Bc7,
    Bc7Srgb,
    Rgba8,
    Rgba8Srgb,
    Bgra8,
    Bgra8Srgb,
}

impl Format {
    /// Байт на блок 4×4 (BCn) или на пиксель (несжатые).
    pub fn block_bytes(self) -> usize {
        match self {
            Self::Bc1 | Self::Bc1Srgb | Self::Bc4 => 8,
            Self::Rgba8 | Self::Rgba8Srgb | Self::Bgra8 | Self::Bgra8Srgb => 4,
            _ => 16,
        }
    }

    pub fn compressed(self) -> bool {
        !matches!(self, Self::Rgba8 | Self::Rgba8Srgb | Self::Bgra8 | Self::Bgra8Srgb)
    }

    /// (байт в строке, байт в уровне) для уровня w×h.
    pub fn level_size(self, w: usize, h: usize) -> (usize, usize) {
        if self.compressed() {
            let (bw, bh) = (w.div_ceil(4).max(1), h.div_ceil(4).max(1));
            (bw * self.block_bytes(), bw * bh * self.block_bytes())
        } else {
            (w * 4, w * h * 4)
        }
    }
}

#[derive(Debug, Clone)]
pub struct TextureData {
    pub format: Format,
    pub width: usize,
    pub height: usize,
    /// Уровни мипов от полного размера.
    pub mips: Vec<Vec<u8>>,
}

fn u32_at(d: &[u8], off: usize) -> Result<u32> {
    d.get(off..off + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).context("DDS обрезан")
}

/// Формат по DXGI (заголовок DX10).
fn dxgi(code: u32) -> Result<Format> {
    Ok(match code {
        71 => Format::Bc1,
        72 => Format::Bc1Srgb,
        74 => Format::Bc2,
        75 => Format::Bc2Srgb,
        77 => Format::Bc3,
        78 => Format::Bc3Srgb,
        80 => Format::Bc4,
        83 => Format::Bc5,
        95 => Format::Bc6hUf,
        98 => Format::Bc7,
        99 => Format::Bc7Srgb,
        28 => Format::Rgba8,
        29 => Format::Rgba8Srgb,
        87 => Format::Bgra8,
        91 => Format::Bgra8Srgb,
        other => bail!("DDS: формат DXGI {other} не поддержан"),
    })
}

/// `srgb` — для старых fourCC (DXT1/3/5), где цветовое пространство не записано.
pub fn parse(d: &[u8], srgb: bool) -> Result<TextureData> {
    if d.get(..4) != Some(b"DDS ") {
        bail!("нет сигнатуры DDS");
    }
    let height = u32_at(d, 12)? as usize;
    let width = u32_at(d, 16)? as usize;
    let mip_count = (u32_at(d, 28)? as usize).max(1);
    let fourcc = d.get(84..88).context("DDS обрезан")?;
    let (format, mut off) = match fourcc {
        b"DX10" => (dxgi(u32_at(d, 128)?)?, 148),
        b"DXT1" => (if srgb { Format::Bc1Srgb } else { Format::Bc1 }, 128),
        b"DXT3" => (if srgb { Format::Bc2Srgb } else { Format::Bc2 }, 128),
        b"DXT5" => (if srgb { Format::Bc3Srgb } else { Format::Bc3 }, 128),
        b"ATI1" | b"BC4U" => (Format::Bc4, 128),
        b"ATI2" | b"BC5U" => (Format::Bc5, 128),
        other => bail!("DDS: fourCC {:?} не поддержан", String::from_utf8_lossy(other)),
    };
    let mut mips = Vec::with_capacity(mip_count);
    let (mut w, mut h) = (width, height);
    for _ in 0..mip_count {
        let (_, size) = format.level_size(w, h);
        let Some(level) = d.get(off..off + size) else { break };
        mips.push(level.to_vec());
        off += size;
        (w, h) = ((w / 2).max(1), (h / 2).max(1));
    }
    if mips.is_empty() {
        bail!("DDS {width}×{height}: нет данных первого уровня");
    }
    Ok(TextureData { format, width, height, mips })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dds(fourcc: &[u8; 4], w: u32, h: u32, mips: u32, payload: usize) -> Vec<u8> {
        let mut d = vec![0u8; 128];
        d[..4].copy_from_slice(b"DDS ");
        d[12..16].copy_from_slice(&h.to_le_bytes());
        d[16..20].copy_from_slice(&w.to_le_bytes());
        d[28..32].copy_from_slice(&mips.to_le_bytes());
        d[84..88].copy_from_slice(fourcc);
        d.extend(std::iter::repeat_n(7u8, payload));
        d
    }

    #[test]
    fn bc1_mip_chain() {
        // 8×8 BC1: 4 блока ×8 = 32 байта, 4×4 → 8, 2×2 → 8, 1×1 → 8.
        let t = parse(&dds(b"DXT1", 8, 8, 4, 32 + 8 + 8 + 8), true).unwrap();
        assert_eq!(t.format, Format::Bc1Srgb);
        assert_eq!(t.mips.iter().map(Vec::len).collect::<Vec<_>>(), vec![32, 8, 8, 8]);
    }

    #[test]
    fn truncated_keeps_available_levels() {
        let t = parse(&dds(b"DXT5", 8, 8, 4, 64), false).unwrap();
        assert_eq!(t.mips.len(), 1);
        assert!(parse(&dds(b"DXT5", 8, 8, 4, 10), false).is_err());
        assert!(parse(b"nope", false).is_err());
    }
}
