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

#[cfg(test)]
mod tests {
    use super::*;

    type Pixel = (u8, u8, u8);
    type Decoded = (u32, u32, Vec<Vec<Pixel>>);

    /// Reads a 24-bit BMP back as (width, height, rows of RGB).
    fn read_bmp(path: &str) -> Decoded {
        let d = std::fs::read(path).unwrap();
        let offset = u32::from_le_bytes(d[10..14].try_into().unwrap()) as usize;
        let width = i32::from_le_bytes(d[18..22].try_into().unwrap()) as u32;
        let height = i32::from_le_bytes(d[22..26].try_into().unwrap()) as u32;
        let stride = ((width * 3).div_ceil(4) * 4) as usize;

        let mut rows = Vec::new();
        // Stored bottom-up, so read back to front to get top-down order.
        for y in (0..height as usize).rev() {
            let start = offset + y * stride;
            let mut row = Vec::new();
            for x in 0..width as usize {
                let p = start + x * 3;
                row.push((d[p + 2], d[p + 1], d[p]));
            }
            rows.push(row);
        }
        (width, height, rows)
    }

    #[test]
    fn writes_pixels_in_the_right_place_and_order() {
        // A 2x2 image with four distinct colours pins down both the row order
        // (BMP is bottom-up) and the channel order (BMP is BGR). Getting
        // either wrong is invisible in a solid-colour test.
        let rgba: Vec<u8> = vec![
            255, 0, 0, 255, // top-left: red
            0, 255, 0, 255, // top-right: green
            0, 0, 255, 255, // bottom-left: blue
            255, 255, 0, 255, // bottom-right: yellow
        ];

        let path = std::env::temp_dir().join("mvr_bmp_test.bmp");
        let path = path.to_str().unwrap();
        write_bmp(path, 2, 2, &rgba).unwrap();

        let (w, h, rows) = read_bmp(path);
        assert_eq!((w, h), (2, 2));
        assert_eq!(rows[0][0], (255, 0, 0), "top-left should stay red");
        assert_eq!(rows[0][1], (0, 255, 0), "top-right should stay green");
        assert_eq!(rows[1][0], (0, 0, 255), "bottom-left should stay blue");
        assert_eq!(rows[1][1], (255, 255, 0), "bottom-right should stay yellow");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn pads_rows_to_a_multiple_of_four_bytes() {
        // A 3-pixel row is 9 bytes and must be padded to 12, or every row
        // after the first lands shifted.
        let rgba = vec![128u8; 3 * 2 * 4];
        let path = std::env::temp_dir().join("mvr_bmp_pad.bmp");
        let path = path.to_str().unwrap();
        write_bmp(path, 3, 2, &rgba).unwrap();

        let (w, h, rows) = read_bmp(path);
        assert_eq!((w, h), (3, 2));
        assert!(rows.iter().all(|r| r.iter().all(|&p| p == (128, 128, 128))));
        let _ = std::fs::remove_file(path);
    }
}
