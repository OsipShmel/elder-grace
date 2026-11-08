//! Склейка двух кадров «слева | справа» в одну PNG (сравнение с эталоном).

use std::path::Path;

use anyhow::{Context, Result, bail};

struct Rgba {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

fn load(path: &Path) -> Result<Rgba> {
    let mut dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).with_context(|| format!("{}", path.display()))?));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().context("размер PNG")?];
    let info = reader.next_frame(&mut buf)?;
    let (w, h) = (info.width as usize, info.height as usize);
    let px = match info.color_type {
        png::ColorType::Rgba => buf[..w * h * 4].to_vec(),
        png::ColorType::Rgb => buf[..w * h * 3].as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..w * h * 2].as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf[..w * h].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        other => bail!("{}: цветовой тип {other:?}", path.display()),
    };
    Ok(Rgba { w, h, px })
}

/// Ближайший сосед — для сравнения на глаз этого достаточно.
fn resize(img: &Rgba, w: usize, h: usize) -> Rgba {
    let mut px = vec![0; w * h * 4];
    for y in 0..h {
        let sy = y * img.h / h;
        for x in 0..w {
            let sx = x * img.w / w;
            let (d, s) = ((y * w + x) * 4, (sy * img.w + sx) * 4);
            px[d..d + 4].copy_from_slice(&img.px[s..s + 4]);
        }
    }
    Rgba { w, h, px }
}

/// `left` и `right` бок о бок; правый приводится к размеру левого.
pub fn side_by_side(left: &Path, right: &Path, out: &Path) -> Result<()> {
    let l = load(left)?;
    let r = resize(&load(right)?, l.w, l.h);
    let (w, h) = (l.w * 2, l.h);
    let mut px = vec![0; w * h * 4];
    for y in 0..h {
        px[y * w * 4..y * w * 4 + l.w * 4].copy_from_slice(&l.px[y * l.w * 4..(y + 1) * l.w * 4]);
        px[y * w * 4 + l.w * 4..(y + 1) * w * 4].copy_from_slice(&r.px[y * r.w * 4..(y + 1) * r.w * 4]);
    }
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(out)?), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&px)?;
    Ok(())
}
