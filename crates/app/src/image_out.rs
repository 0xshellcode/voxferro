//! Minimal BMP writer.
//!
//! BMP rather than PNG to keep the dependency list honest: an uncompressed
//! bitmap is a 54-byte header plus rows of pixels, where PNG would pull in a
//! deflate implementation for a debug convenience.

use std::io::Write;

/// Writes RGBA8 pixels as a 24-bit BMP.
pub fn write_bmp(path: &str, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    let row_unpadded = width * 3;
    // BMP pads every row to a multiple of four bytes.
    let row_padded = row_unpadded.div_ceil(4) * 4;
    let pixel_bytes = row_padded * height;
    const HEADER: u32 = 54;

    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);

    out.write_all(b"BM")?;
    out.write_all(&(HEADER + pixel_bytes).to_le_bytes())?;
    out.write_all(&0u16.to_le_bytes())?; // reserved
    out.write_all(&0u16.to_le_bytes())?;
    out.write_all(&HEADER.to_le_bytes())?; // pixel data offset

    out.write_all(&40u32.to_le_bytes())?; // DIB header size
    out.write_all(&(width as i32).to_le_bytes())?;
    out.write_all(&(height as i32).to_le_bytes())?;
    out.write_all(&1u16.to_le_bytes())?; // planes
    out.write_all(&24u16.to_le_bytes())?; // bits per pixel
    out.write_all(&0u32.to_le_bytes())?; // BI_RGB, no compression
    out.write_all(&pixel_bytes.to_le_bytes())?;
    out.write_all(&2835u32.to_le_bytes())?; // 72 DPI, horizontal
    out.write_all(&2835u32.to_le_bytes())?; // vertical
    out.write_all(&0u32.to_le_bytes())?; // palette colours used
    out.write_all(&0u32.to_le_bytes())?; // important colours

    // BMP rows run bottom-up, and pixels are BGR rather than RGB.
    let padding = vec![0u8; (row_padded - row_unpadded) as usize];
    for y in (0..height).rev() {
        let start = (y * width * 4) as usize;
        for x in 0..width as usize {
            let p = start + x * 4;
            out.write_all(&[rgba[p + 2], rgba[p + 1], rgba[p]])?;
        }
        out.write_all(&padding)?;
    }
    out.flush()
}
