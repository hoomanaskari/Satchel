//! PNG thumbnails, made once at capture time.
//!
//! Non-interlaced images are decoded row by row and box-filtered straight
//! into the thumbnail accumulator, so a 20 MB screenshot never needs a full
//! RGBA buffer in memory. Interlaced images (rare on the clipboard) need the
//! whole frame and are only accepted up to a smaller size.

use std::io::Cursor;

use png::{BitDepth, ColorType, Decoder, Limits, Transformations};

/// Images with more pixels than this are not thumbnailed (and not stored).
pub const MAX_PIXELS: u64 = 100_000_000;
/// Upper bound for the full-frame buffer of interlaced images.
pub const MAX_INTERLACED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum ThumbnailError {
    Decode,
    TooLarge,
    Encode,
}

/// Target size that fits in `max_side` x `max_side`, keeping the aspect
/// ratio. Never upscales; never returns a zero dimension.
pub fn fit(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= max_side {
        return (width.max(1), height.max(1));
    }
    let scale = |v: u32| -> u32 {
        let scaled = (v as u64 * max_side as u64 + longest as u64 / 2) / longest as u64;
        (scaled as u32).clamp(1, max_side)
    };
    (scale(width), scale(height))
}

/// Box-filter accumulator with premultiplied alpha.
struct Accumulator {
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    /// Per destination pixel: premultiplied r, g, b sums, alpha sum, count.
    sums: Vec<[u64; 5]>,
}

impl Accumulator {
    fn new(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Self {
        Self {
            src_w,
            src_h,
            dst_w,
            dst_h,
            sums: vec![[0; 5]; dst_w as usize * dst_h as usize],
        }
    }

    /// Adds one source row given as RGBA pixels via `pixel(x)`.
    fn add_row(&mut self, y: u32, pixel: impl Fn(usize) -> [u8; 4]) {
        let ty = (y as u64 * self.dst_h as u64 / self.src_h as u64) as usize;
        let row_base = ty * self.dst_w as usize;
        for x in 0..self.src_w as usize {
            let tx = (x as u64 * self.dst_w as u64 / self.src_w as u64) as usize;
            let [r, g, b, a] = pixel(x);
            let a64 = a as u64;
            let cell = &mut self.sums[row_base + tx];
            cell[0] += r as u64 * a64;
            cell[1] += g as u64 * a64;
            cell[2] += b as u64 * a64;
            cell[3] += a64;
            cell[4] += 1;
        }
    }

    fn finish(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.sums.len() * 4);
        for [r, g, b, a, n] in self.sums {
            if a == 0 || n == 0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                out.push((r / a) as u8);
                out.push((g / a) as u8);
                out.push((b / a) as u8);
                out.push((a / n) as u8);
            }
        }
        out
    }
}

/// Converts one decoded (8-bit, expanded) pixel to RGBA.
fn rgba(color: ColorType, row: &[u8], x: usize) -> [u8; 4] {
    match color {
        ColorType::Grayscale => {
            let v = row[x];
            [v, v, v, 255]
        }
        ColorType::GrayscaleAlpha => {
            let v = row[x * 2];
            [v, v, v, row[x * 2 + 1]]
        }
        ColorType::Rgb => [row[x * 3], row[x * 3 + 1], row[x * 3 + 2], 255],
        // `EXPAND` turns indexed images into RGB(A); treat leftovers as RGBA.
        ColorType::Rgba | ColorType::Indexed => {
            [row[x * 4], row[x * 4 + 1], row[x * 4 + 2], row[x * 4 + 3]]
        }
    }
}

fn bytes_per_pixel(color: ColorType) -> usize {
    match color {
        ColorType::Grayscale => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Rgb => 3,
        ColorType::Rgba | ColorType::Indexed => 4,
    }
}

/// Decodes `png` and returns a PNG thumbnail whose longest side is at most
/// `max_side`.
pub fn png_thumbnail(png: &[u8], max_side: u32) -> Result<Vec<u8>, ThumbnailError> {
    let mut decoder = Decoder::new_with_limits(
        Cursor::new(png),
        Limits { bytes: MAX_INTERLACED_BYTES + 1024 * 1024 },
    );
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    // The header alone is enough to refuse absurd sizes before any pixel
    // data is inflated.
    let (width, height, interlaced) = {
        let info = decoder.read_header_info().map_err(|_| ThumbnailError::Decode)?;
        (info.width, info.height, info.interlaced)
    };
    if width == 0 || height == 0 {
        return Err(ThumbnailError::Decode);
    }
    if width as u64 * height as u64 > MAX_PIXELS {
        return Err(ThumbnailError::TooLarge);
    }
    let mut reader = decoder.read_info().map_err(|_| ThumbnailError::Decode)?;
    let (color, depth) = reader.output_color_type();
    if depth != BitDepth::Eight {
        return Err(ThumbnailError::Decode);
    }
    let bpp = bytes_per_pixel(color);
    let row_len = width as usize * bpp;
    let (dst_w, dst_h) = fit(width, height, max_side);
    let mut acc = Accumulator::new(width, height, dst_w, dst_h);

    if interlaced {
        let size = reader.output_buffer_size().ok_or(ThumbnailError::TooLarge)?;
        if size > MAX_INTERLACED_BYTES {
            return Err(ThumbnailError::TooLarge);
        }
        let mut frame = vec![0; size];
        let info = reader.next_frame(&mut frame).map_err(|_| ThumbnailError::Decode)?;
        let line = info.line_size;
        for y in 0..height {
            let start = y as usize * line;
            let row = frame.get(start..start + row_len).ok_or(ThumbnailError::Decode)?;
            acc.add_row(y, |x| rgba(color, row, x));
        }
    } else {
        for y in 0..height {
            let row = reader
                .next_row()
                .map_err(|_| ThumbnailError::Decode)?
                .ok_or(ThumbnailError::Decode)?;
            let data = row.data();
            if data.len() < row_len {
                return Err(ThumbnailError::Decode);
            }
            acc.add_row(y, |x| rgba(color, data, x));
        }
    }

    encode_rgba(&acc.finish(), dst_w, dst_h)
}

pub fn encode_rgba(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, ThumbnailError> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|_| ThumbnailError::Encode)?;
        writer.write_image_data(rgba).map_err(|_| ThumbnailError::Encode)?;
        writer.finish().map_err(|_| ThumbnailError::Encode)?;
    }
    Ok(out)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A width x height RGBA PNG: left half opaque red, right half
    /// transparent.
    pub fn png(width: u32, height: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _y in 0..height {
            for x in 0..width {
                if x < width / 2 {
                    rgba.extend_from_slice(&[255, 0, 0, 255]);
                } else {
                    rgba.extend_from_slice(&[0, 0, 255, 0]);
                }
            }
        }
        encode_rgba(&rgba, width, height).unwrap()
    }

    pub fn decode(png: &[u8]) -> (u32, u32, Vec<u8>) {
        let decoder = Decoder::new(Cursor::new(png));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        (info.width, info.height, buf)
    }

    pub fn dimensions(png: &[u8]) -> (u32, u32) {
        let (w, h, _) = decode(png);
        (w, h)
    }

    #[test]
    fn fit_keeps_aspect_and_never_upscales() {
        assert_eq!(fit(640, 320, 320), (320, 160));
        assert_eq!(fit(320, 640, 320), (160, 320));
        assert_eq!(fit(100, 50, 320), (100, 50));
        assert_eq!(fit(100_000, 1, 320), (320, 1));
        assert_eq!(fit(1, 100_000, 320), (1, 320));
        assert_eq!(fit(1000, 1000, 320), (320, 320));
    }

    #[test]
    fn downscales_and_preserves_colors_and_alpha() {
        let thumb = png_thumbnail(&png(64, 32), 16).unwrap();
        let (w, h, px) = decode(&thumb);
        assert_eq!((w, h), (16, 8));
        // First pixel: opaque red. Last pixel: fully transparent.
        assert_eq!(&px[0..4], &[255, 0, 0, 255]);
        assert_eq!(px[px.len() - 1], 0);
    }

    #[test]
    fn small_images_keep_their_size() {
        let thumb = png_thumbnail(&png(10, 6), 320).unwrap();
        assert_eq!(dimensions(&thumb), (10, 6));
    }

    #[test]
    fn rgb_and_grayscale_inputs() {
        for (color, bpp) in [(ColorType::Rgb, 3usize), (ColorType::Grayscale, 1), (ColorType::GrayscaleAlpha, 2)] {
            let (w, h) = (40u32, 20u32);
            let data = vec![200u8; w as usize * h as usize * bpp];
            let mut out = Vec::new();
            {
                let mut enc = png::Encoder::new(&mut out, w, h);
                enc.set_color(color);
                enc.set_depth(BitDepth::Eight);
                let mut writer = enc.write_header().unwrap();
                writer.write_image_data(&data).unwrap();
                writer.finish().unwrap();
            }
            let (tw, th, px) = decode(&png_thumbnail(&out, 10).unwrap());
            assert_eq!((tw, th), (10, 5), "{color:?}");
            assert_eq!(px[0], 200, "{color:?}");
        }
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }

    /// Hand-built Adam7-interlaced RGBA PNG of one solid color (the `png`
    /// encoder cannot write interlaced files). Uses stored deflate blocks.
    fn interlaced_png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        const PASSES: [(u32, u32, u32, u32); 7] =
            [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];
        let mut raw = Vec::new();
        for (x0, y0, dx, dy) in PASSES {
            let pw = w.saturating_sub(x0).div_ceil(dx);
            let ph = h.saturating_sub(y0).div_ceil(dy);
            if pw == 0 || ph == 0 {
                continue;
            }
            let mut row = vec![0u8];
            for _ in 0..pw {
                row.extend_from_slice(&rgba);
            }
            for _ in 0..ph {
                raw.extend_from_slice(&row);
            }
        }
        let mut z = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
        for (i, block) in blocks.iter().enumerate() {
            z.push((i + 1 == blocks.len()) as u8);
            let len = block.len() as u16;
            z.extend_from_slice(&len.to_le_bytes());
            z.extend_from_slice(&(!len).to_le_bytes());
            z.extend_from_slice(block);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in &raw {
            a = (a + byte as u32) % 65_521;
            b = (b + a) % 65_521;
        }
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());

        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 1]);
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &z);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn interlaced_images_are_supported() {
        let png = interlaced_png(50, 30, [90, 90, 90, 90]);
        let (w, h, _) = decode(&png);
        assert_eq!((w, h), (50, 30));
        let (tw, th, px) = decode(&png_thumbnail(&png, 25).unwrap());
        assert_eq!((tw, th), (25, 15));
        assert_eq!(&px[0..4], &[90, 90, 90, 90]);
    }

    #[test]
    fn garbage_and_truncated_input_fail_cleanly() {
        assert_eq!(png_thumbnail(b"", 320), Err(ThumbnailError::Decode));
        assert_eq!(png_thumbnail(b"definitely not a png", 320), Err(ThumbnailError::Decode));
        let good = png(64, 64);
        assert!(png_thumbnail(&good[..good.len() / 2], 320).is_err());
    }

    #[test]
    fn absurd_dimensions_are_refused_before_decoding() {
        // Header claims 20000 x 20000 = 400 MP; there is no pixel data.
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 20_000, 20_000);
            enc.set_color(ColorType::Grayscale);
            enc.set_depth(BitDepth::Eight);
            let _ = enc.write_header().unwrap();
        }
        assert_eq!(png_thumbnail(&out, 320), Err(ThumbnailError::TooLarge));
    }
}
