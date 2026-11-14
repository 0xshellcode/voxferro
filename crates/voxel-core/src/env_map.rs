//! Environment maps: reading Radiance `.hdr` files and preparing them for
//! importance sampling.

use std::fmt;

/// An equirectangular HDR image: row 0 is straight up (+Z), the last row
/// straight down, and each row wraps once around the horizon.
#[derive(Debug, Clone)]
pub struct EnvMap {
    pub width: u32,
    pub height: u32,
    /// Linear RGB, row-major.
    pub pixels: Vec<[f32; 3]>,
}

#[derive(Debug)]
pub enum HdrError {
    NotRadiance,
    UnsupportedFormat(String),
    BadResolution(String),
    Truncated,
    BadScanline(&'static str),
}

impl fmt::Display for HdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRadiance => write!(f, "not a Radiance HDR file"),
            Self::UnsupportedFormat(s) => write!(f, "unsupported pixel format {s}"),
            Self::BadResolution(s) => write!(f, "unsupported resolution line {s:?}"),
            Self::Truncated => write!(f, "file ends before the image does"),
            Self::BadScanline(why) => write!(f, "corrupt scanline: {why}"),
        }
    }
}

impl std::error::Error for HdrError {}

/// RGBE to linear RGB: a shared 8-bit exponent over three 8-bit mantissas.
fn rgbe_to_rgb([r, g, b, e]: [u8; 4]) -> [f32; 3] {
    if e == 0 {
        return [0.0; 3];
    }
    // ldexp(1, e - (128 + 8)): the mantissas are 8-bit fractions.
    let scale = f32::powi(2.0, e as i32 - 136);
    [r as f32 * scale, g as f32 * scale, b as f32 * scale]
}

impl EnvMap {
    /// Parses a Radiance `.hdr` file.
    ///
    /// Supports the run-length encoded scanlines every modern writer emits
    /// plus flat RGBE. The pre-1991 "old"
    /// RLE variant is rejected rather than misread.
    pub fn parse_hdr(data: &[u8]) -> Result<Self, HdrError> {
        if !(data.starts_with(b"#?RADIANCE") || data.starts_with(b"#?RGBE")) {
            return Err(HdrError::NotRadiance);
        }

        // Header: text lines up to an empty one, then the resolution line.
        let mut pos = 0usize;
        let next_line = |pos: &mut usize| -> Option<&[u8]> {
            let start = *pos;
            let end = data[start..].iter().position(|&c| c == b'\n')? + start;
            *pos = end + 1;
            Some(&data[start..end])
        };
        loop {
            let line = next_line(&mut pos).ok_or(HdrError::Truncated)?;
            if line.is_empty() {
                break;
            }
            if let Some(format) = line.strip_prefix(b"FORMAT=")
                && format != b"32-bit_rle_rgbe" {
                    return Err(HdrError::UnsupportedFormat(
                        String::from_utf8_lossy(format).into_owned(),
                    ));
                }
        }
        let resolution = next_line(&mut pos).ok_or(HdrError::Truncated)?;
        let text = String::from_utf8_lossy(resolution).into_owned();
        let parts: Vec<&str> = text.split_whitespace().collect();
        // Only the standard top-down orientation: "-Y height +X width".
        let (height, width) = match parts.as_slice() {
            ["-Y", h, "+X", w] => (
                h.parse::<u32>().map_err(|_| HdrError::BadResolution(text.clone()))?,
                w.parse::<u32>().map_err(|_| HdrError::BadResolution(text.clone()))?,
            ),
            _ => return Err(HdrError::BadResolution(text)),
        };
        if width == 0 || height == 0 || width > 1 << 15 || height > 1 << 15 {
            return Err(HdrError::BadResolution(text));
        }

        let mut pixels = Vec::with_capacity((width * height) as usize);
        let mut scanline = vec![[0u8; 4]; width as usize];
        let body = &data[pos..];
        let mut at = 0usize;

        for _ in 0..height {
            let rle = (8..0x8000).contains(&width)
                && body.len() >= at + 4
                && body[at] == 2
                && body[at + 1] == 2
                && body[at + 2] & 0x80 == 0;

            if rle {
                let declared = ((body[at + 2] as u32) << 8) | body[at + 3] as u32;
                if declared != width {
                    return Err(HdrError::BadScanline("width disagrees with the header"));
                }
                at += 4;
                // Each of the four channels is run-length encoded separately.
                for channel in 0..4 {
                    let mut x = 0usize;
                    while x < width as usize {
                        let count = *body.get(at).ok_or(HdrError::Truncated)? as usize;
                        at += 1;
                        if count > 128 {
                            let run = count - 128;
                            let value = *body.get(at).ok_or(HdrError::Truncated)?;
                            at += 1;
                            if x + run > width as usize {
                                return Err(HdrError::BadScanline("run overflows the row"));
                            }
                            for px in &mut scanline[x..x + run] {
                                px[channel] = value;
                            }
                            x += run;
                        } else {
                            if count == 0 || x + count > width as usize {
                                return Err(HdrError::BadScanline("literal overflows the row"));
                            }
                            let bytes = body.get(at..at + count).ok_or(HdrError::Truncated)?;
                            for (px, &v) in scanline[x..x + count].iter_mut().zip(bytes) {
                                px[channel] = v;
                            }
                            at += count;
                            x += count;
                        }
                    }
                }
            } else {
                // Flat RGBE. A first pixel of (1,1,1,n) would mark old-style
                // RLE, which nothing current writes; refuse it explicitly.
                if body.len() >= at + 4 && body[at..at + 3] == [1, 1, 1] {
                    return Err(HdrError::UnsupportedFormat("old-style RLE".to_owned()));
                }
                let bytes = body
                    .get(at..at + 4 * width as usize)
                    .ok_or(HdrError::Truncated)?;
                for (px, chunk) in scanline.iter_mut().zip(bytes.as_chunks::<4>().0) {
                    *px = *chunk;
                }
                at += 4 * width as usize;
            }
            pixels.extend(scanline.iter().map(|&p| rgbe_to_rgb(p)));
        }

        Ok(Self {
            width,
            height,
            pixels,
        })
    }
}

/// Importance-sampling tables for an environment map.
#[derive(Debug, Clone)]
pub struct EnvSampling {
    /// Running sum of per-texel weights over the whole image, row-major.
    pub cdf: Vec<f32>,
    /// Sum of all weights: the last CDF entry.
    pub total: f32,
    /// Integral of the weight over the sphere, `∫ w dω`.
    pub integral: f32,
}

impl EnvSampling {
    /// Builds the tables.
    ///
    /// The weight is `max(r, g, b)`, not luminance, and the CDF is a flat
    /// prefix sum with no sin θ factor -- both as the editor does it. The
    /// solid-angle correction instead lives in the integral, where each row
    /// contributes over its true band area `(2π / W)(cos θ₀ − cos θ₁)`, and
    /// in the sampling pdf on the GPU.
    pub fn new(map: &EnvMap) -> Self {
        let (w, h) = (map.width as usize, map.height as usize);
        let weight = |p: &[f32; 3]| p[0].max(p[1]).max(p[2]);

        let mut integral = 0.0f64;
        let mut cos_above = 1.0f64;
        for row in 0..h {
            let row_sum: f64 = map.pixels[row * w..(row + 1) * w]
                .iter()
                .map(|p| weight(p) as f64)
                .sum();
            let cos_below = (std::f64::consts::PI * (row + 1) as f64 / h as f64).cos();
            integral += row_sum * (cos_above - cos_below);
            cos_above = cos_below;
        }
        integral *= std::f64::consts::TAU / w as f64;

        let mut cdf = Vec::with_capacity(w * h);
        let mut running = 0.0f32;
        for p in &map.pixels {
            running += weight(p);
            cdf.push(running);
        }

        Self {
            total: running,
            cdf,
            integral: integral as f32,
        }
    }

    /// The radiance multiplier that makes the map's mean weight equal
    /// `intensity`: `intensity · 4π / ∫ w dω`. So a
    /// file's `_i` means the same whatever the HDR's absolute exposure.
    pub fn scale(&self, intensity: f32) -> f32 {
        if self.integral <= 0.0 {
            return 0.0;
        }
        intensity * 4.0 * std::f32::consts::PI / self.integral
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(width: u32, height: u32, value: [f32; 3]) -> EnvMap {
        EnvMap {
            width,
            height,
            pixels: vec![value; (width * height) as usize],
        }
    }

    #[test]
    fn rgbe_decodes_the_shared_exponent() {
        assert_eq!(rgbe_to_rgb([0, 0, 0, 0]), [0.0; 3]);
        // Mantissa 128 with exponent 129 is 128 * 2^(129-136) = 1.0.
        assert_eq!(rgbe_to_rgb([128, 64, 0, 129]), [1.0, 0.5, 0.0]);
    }

    #[test]
    fn a_uniform_sky_integrates_to_four_pi() {
        // Rows cover cos θ from 1 to -1, a total of 2; times 2π per row
        // width gives 4π for a weight of 1 everywhere.
        let s = EnvSampling::new(&flat(64, 32, [1.0, 1.0, 1.0]));
        assert!((s.integral - 4.0 * std::f32::consts::PI).abs() < 1e-3);
        assert!((s.scale(0.7) - 0.7).abs() < 1e-5, "mean radiance becomes _i");
    }

    #[test]
    fn weight_is_the_brightest_channel() {
        let s = EnvSampling::new(&flat(4, 2, [0.1, 0.9, 0.3]));
        assert!((s.total - 0.9 * 8.0).abs() < 1e-5);
    }

    #[test]
    fn the_cdf_is_monotonic() {
        let mut m = flat(8, 4, [0.5, 0.5, 0.5]);
        m.pixels[5] = [10.0, 0.0, 0.0];
        let s = EnvSampling::new(&m);
        assert!(s.cdf.windows(2).all(|w| w[1] >= w[0]));
        assert_eq!(*s.cdf.last().unwrap(), s.total);
    }

    #[test]
    fn rejects_files_that_are_not_radiance() {
        assert!(matches!(EnvMap::parse_hdr(b"P6\n1 1\n255\n"), Err(HdrError::NotRadiance)));
    }

    #[test]
    fn reads_a_tiny_flat_file() {
        let mut data = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 2\n".to_vec();
        data.extend_from_slice(&[128, 0, 0, 129, 0, 128, 0, 129]);
        let m = EnvMap::parse_hdr(&data).unwrap();
        assert_eq!((m.width, m.height), (2, 1));
        assert_eq!(m.pixels, vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    }
}
