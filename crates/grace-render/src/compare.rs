//! Joins two frames "left | right" into one PNG (comparison with the oracle).

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
    let mut buf = vec![0; reader.output_buffer_size().context("PNG size")?];
    let info = reader.next_frame(&mut buf)?;
    let (w, h) = (info.width as usize, info.height as usize);
    let px = match info.color_type {
        png::ColorType::Rgba => buf[..w * h * 4].to_vec(),
        png::ColorType::Rgb => buf[..w * h * 3].as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..w * h * 2].as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf[..w * h].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        other => bail!("{}: color type {other:?}", path.display()),
    };
    Ok(Rgba { w, h, px })
}

/// Nearest neighbor: good enough for comparing by eye.
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

/// Center crop to the `w:h` aspect ratio (black window bars around the game frame).
fn crop_to_aspect(img: Rgba, w: usize, h: usize) -> Rgba {
    let (cw, ch) = if img.w * h > img.h * w { (img.h * w / h, img.h) } else { (img.w, img.w * h / w) };
    if (cw, ch) == (img.w, img.h) {
        return img;
    }
    let (x0, y0) = ((img.w - cw) / 2, (img.h - ch) / 2);
    let mut px = Vec::with_capacity(cw * ch * 4);
    for y in y0..y0 + ch {
        px.extend_from_slice(&img.px[(y * img.w + x0) * 4..(y * img.w + x0 + cw) * 4]);
    }
    Rgba { w: cw, h: ch, px }
}

/// `left` and `right` side by side; the right one is center-cropped to the left one's aspect and scaled to its size.
pub fn side_by_side(left: &Path, right: &Path, out: &Path) -> Result<()> {
    let l = load(left)?;
    let r = resize(&crop_to_aspect(load(right)?, l.w, l.h), l.w, l.h);
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
