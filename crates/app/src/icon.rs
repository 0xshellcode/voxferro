//! The app icon: a render cut to the macOS icon shape.
//!
//! `voxferro assets/icon/icon.vox --render N --icon out.png` renders the icon
//! model with the path tracer and writes it here: the centre square of the
//! frame, scaled to 1024 px, inside the rounded square of Apple's icon grid
//! (an 824 px body with 100 px of transparent margin). The corners are a
//! superellipse, which approaches the continuous curve of the system's own
//! icons better than a circular arc.
//!
//! Next to it goes `<name>-square.png`, the same render filling the whole
//! canvas. That one is for the `.icns`: since macOS 26 the system cuts
//! bundle icons to its own shape, and an icon that brings its own shape and
//! margins is shrunk into a grey frame instead. The shaped one is what the
//! window sets at run time, where nothing reshapes it.

/// Side of the written icon, the largest size an `.icns` holds.
pub const SIZE: u32 = 1024;
/// The body of the icon inside the canvas, per Apple's grid.
const BODY: f32 = 824.0;
/// Superellipse exponent: 2 is a circle, larger is squarer.
const EXPONENT: f32 = 5.0;

/// The icon as RGBA, from a frame of `width` x `height` RGBA pixels:
/// `shaped` inside the rounded square, else filling the canvas.
pub fn icon_rgba(width: u32, height: u32, rgba: &[u8], shaped: bool) -> Vec<u8> {
    let side = width.min(height) as f32;
    let (x0, y0) = ((width as f32 - side) / 2.0, (height as f32 - side) / 2.0);
    // Source pixels per icon pixel, measured over the body, which shows the
    // whole centre square.
    let body = if shaped { BODY } else { SIZE as f32 };
    let scale = side / body;
    let margin = (SIZE as f32 - body) / 2.0;

    let mut out = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let coverage = if shaped { coverage(x, y) } else { 1.0 };
            if coverage == 0.0 {
                continue;
            }
            // Box filter over the source pixels this one covers.
            let sx0 = x0 + (x as f32 - margin) * scale;
            let sy0 = y0 + (y as f32 - margin) * scale;
            let (sx1, sy1) = (sx0 + scale, sy0 + scale);
            let mut sum = [0.0f32; 3];
            let mut n = 0.0;
            let clampx = |v: f32| (v.max(0.0) as u32).min(width - 1);
            let clampy = |v: f32| (v.max(0.0) as u32).min(height - 1);
            for sy in clampy(sy0)..=clampy(sy1 - 0.001) {
                for sx in clampx(sx0)..=clampx(sx1 - 0.001) {
                    let i = ((sy * width + sx) * 4) as usize;
                    for c in 0..3 {
                        sum[c] += rgba[i + c] as f32;
                    }
                    n += 1.0;
                }
            }
            let o = ((y * SIZE + x) * 4) as usize;
            for c in 0..3 {
                out[o + c] = (sum[c] / n).round() as u8;
            }
            out[o + 3] = (coverage * 255.0).round() as u8;
        }
    }
    out
}

/// How much of icon pixel (x, y) lies inside the body, 4x4 supersampled.
fn coverage(x: u32, y: u32) -> f32 {
    let half = BODY / 2.0;
    let centre = SIZE as f32 / 2.0;
    let mut inside = 0;
    for j in 0..4 {
        for i in 0..4 {
            let px = (x as f32 + (i as f32 + 0.5) / 4.0 - centre) / half;
            let py = (y as f32 + (j as f32 + 0.5) / 4.0 - centre) / half;
            if px.abs().powf(EXPONENT) + py.abs().powf(EXPONENT) <= 1.0 {
                inside += 1;
            }
        }
    }
    inside as f32 / 16.0
}

/// Writes the shaped icon to `path` and the full-bleed one next to it.
pub fn write(path: &str, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    let png = |shaped| voxel_core::export::png_rgba(SIZE, SIZE, &icon_rgba(width, height, rgba, shaped));
    std::fs::write(path, png(true))?;
    let square = match path.strip_suffix(".png") {
        Some(stem) => format!("{stem}-square.png"),
        None => format!("{path}-square.png"),
    };
    std::fs::write(square, png(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_is_opaque_and_the_margin_clear() {
        let frame = vec![200u8; 300 * 200 * 4];
        let icon = icon_rgba(300, 200, &frame, true);
        let alpha = |x: u32, y: u32| icon[((y * SIZE + x) * 4 + 3) as usize];
        assert_eq!(alpha(512, 512), 255);
        assert_eq!(alpha(512, 105), 255, "edge midpoint is inside");
        assert_eq!(alpha(50, 512), 0, "margin");
        assert_eq!(alpha(102, 102), 0, "rounded corner");
        assert_eq!(icon[(512 * SIZE + 512) as usize * 4], 200);
        let square = icon_rgba(300, 200, &frame, false);
        assert!(square.chunks(4).all(|p| p[3] == 255), "full bleed");
    }
}
