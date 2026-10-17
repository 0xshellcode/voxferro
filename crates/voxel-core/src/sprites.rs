//! the reference editor's *iso* and *2d* exports: the model as pixel-art sprites.
//!
//! From `the reference renderer` and the `io_iso` and `io_2d` options of its
//! `config.txt`: each voxel is a small isometric cube placed at
//! `x = (vx - vy) · A`, `y = (vx + vy) · B - vz · H` (A the half width of a
//! cube's top, B its half height, H its side height -- `size` and `height`
//! in the options), its top and two sides lit by the `top`, `left` and
//! `right` factors, clamped; up to four views, a quarter turn apart, side by
//! side; and an outline. The pixel rules within a cube, the bevel highlight
//! and the 2D sprite's shadow are this editor's reading of the options, not
//! a transcription: each cube is rasterised from its exact outline, drawn
//! back to front.

use crate::{Volume, ops};
use vox_format::Palette;

/// `io_iso` in the reference editor's `config.txt`, with its defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IsoOptions {
    /// Pixels: a cube's top is `4 · size` wide (1 to 8).
    pub size: u32,
    /// Pixels of a cube's side (1 to 24).
    pub height: u32,
    /// Views, a quarter turn apart (1 to 4).
    pub views: u32,
    pub outline: bool,
    pub top: [f32; 3],
    pub left: [f32; 3],
    pub right: [f32; 3],
}

impl Default for IsoOptions {
    fn default() -> Self {
        Self {
            size: 2,
            height: 4,
            views: 4,
            outline: true,
            top: [1.17, 1.15, 1.25],
            left: [0.37, 0.35, 0.55],
            right: [0.87, 0.85, 0.95],
        }
    }
}

/// `io_2d`, with its defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlatOptions {
    /// Pixels per voxel (1 to 32).
    pub size: u32,
    pub shadow: bool,
    pub outline: bool,
}

impl Default for FlatOptions {
    fn default() -> Self {
        Self { size: 16, shadow: true, outline: false }
    }
}

/// An RGBA image.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    fn new(width: u32, height: u32) -> Self {
        Self { width, height, rgba: vec![0; (width * height * 4) as usize] }
    }

    fn put(&mut self, x: i32, y: i32, c: [u8; 4]) {
        if x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height {
            let at = ((y as u32 * self.width + x as u32) * 4) as usize;
            self.rgba[at..at + 4].copy_from_slice(&c);
        }
    }

    fn alpha(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return 0;
        }
        self.rgba[((y as u32 * self.width + x as u32) * 4 + 3) as usize]
    }

    /// Darkens the opaque pixels that touch transparency: the outline.
    fn outline(&mut self, factor: f32) {
        let mut edge = Vec::new();
        for y in 0..self.height as i32 {
            for x in 0..self.width as i32 {
                if self.alpha(x, y) != 0 && [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| self.alpha(x + dx, y + dy) == 0) {
                    edge.push((x, y));
                }
            }
        }
        for (x, y) in edge {
            let at = ((y as u32 * self.width + x as u32) * 4) as usize;
            for c in &mut self.rgba[at..at + 3] {
                *c = (*c as f32 * factor) as u8;
            }
        }
    }

    /// `other` placed to the right of this one.
    fn beside(self, other: Image) -> Image {
        let mut out = Image::new(self.width + other.width, self.height.max(other.height));
        for (img, x0) in [(&self, 0), (&other, self.width)] {
            for y in 0..img.height {
                let from = (y * img.width * 4) as usize;
                let to = ((y * out.width + x0) * 4) as usize;
                out.rgba[to..to + (img.width * 4) as usize].copy_from_slice(&img.rgba[from..from + (img.width * 4) as usize]);
            }
        }
        out
    }
}

fn lit(c: vox_format::Color, k: [f32; 3]) -> [u8; 4] {
    let f = |v: u8, k: f32| (v as f32 * k).clamp(0.0, 255.0) as u8;
    [f(c.r, k[0]), f(c.g, k[1]), f(c.b, k[2]), 255]
}

/// One isometric view of `volume`.
fn iso_view(volume: &Volume, palette: &Palette, o: &IsoOptions) -> Image {
    let a = (2 * o.size.clamp(1, 8)) as i32;
    let b = a / 2;
    let h = o.height.clamp(1, 24) as i32;
    let [sx, sy, sz] = volume.size().map(|s| s as i32);
    let width = ((sx + sy) * a).max(1) as u32;
    let height = ((sx + sy) * b + sz * h).max(1) as u32;
    let mut img = Image::new(width, height);
    // Screen place of a cube's top corner; the image's left edge is the
    // left corner of the voxel at (0, sy - 1).
    let ox = sy * a;
    let oy = (sz - 1) * h;
    // Back to front: farther rows first, lower layers first.
    for sum in 0..(sx + sy - 1) {
        for z in 0..sz {
            for x in 0..sx {
                let y = sum - x;
                if !(0..sy).contains(&y) {
                    continue;
                }
                let v = volume.get(x as u32, y as u32, z as u32);
                if v == 0 {
                    continue;
                }
                let c = palette.color(v);
                let cx = (x - y) * a + ox;
                let cy = (x + y) * b - z * h + oy;
                let (top, left, right) = (lit(c, o.top), lit(c, o.left), lit(c, o.right));
                for px in -a..a {
                    let fx = px as f32 + 0.5;
                    // The top's lower edge at this column.
                    let edge = if fx < 0.0 { b as f32 + (fx + a as f32) * b as f32 / a as f32 } else { 2.0 * b as f32 - fx * b as f32 / a as f32 };
                    // The top is a diamond: its upper edge mirrors the lower.
                    let upper = 2.0 * b as f32 - edge;
                    for py in 0..(2 * b + h) {
                        let fy = py as f32 + 0.5;
                        let colour = if fy >= upper && fy < edge {
                            top
                        } else if fy >= edge && fy < edge + h as f32 {
                            if fx < 0.0 { left } else { right }
                        } else {
                            continue;
                        };
                        img.put(cx + px, cy + py, colour);
                    }
                }
            }
        }
    }
    if o.outline {
        img.outline(0.55);
    }
    img
}

/// the reference editor's *iso* export: `views` isometric views side by side, each a
/// quarter turn on from the last.
pub fn iso(volume: &Volume, palette: &Palette, o: &IsoOptions) -> Image {
    let mut turned = volume.clone();
    let mut out = iso_view(&turned, palette, o);
    for _ in 1..o.views.clamp(1, 4) {
        turned = ops::rotate(&turned, 2);
        out = out.beside(iso_view(&turned, palette, o));
    }
    out
}

/// the reference editor's *2d* export: the model from the front (looking along +y),
/// `size` pixels a voxel, the nearest voxel of each column; with a shadow,
/// a darker copy of the silhouette one voxel down and to the right behind
/// it; with an outline, the silhouette's edge darkened.
pub fn flat(volume: &Volume, palette: &Palette, o: &FlatOptions) -> Image {
    let s = o.size.clamp(1, 32) as i32;
    let [sx, sy, sz] = volume.size().map(|v| v as i32);
    let pad = if o.shadow { s } else { 0 };
    let mut img = Image::new((sx * s + pad) as u32, (sz * s + pad) as u32);
    let front = |x: i32, z: i32| (0..sy).map(|y| volume.get(x as u32, y as u32, z as u32)).find(|&v| v != 0);
    if o.shadow {
        for z in 0..sz {
            for x in 0..sx {
                if front(x, z).is_some() {
                    for py in 0..s {
                        for px in 0..s {
                            img.put(x * s + px + pad, (sz - 1 - z) * s + py + pad, [0, 0, 0, 90]);
                        }
                    }
                }
            }
        }
    }
    for z in 0..sz {
        for x in 0..sx {
            if let Some(v) = front(x, z) {
                let c = palette.color(v);
                for py in 0..s {
                    for px in 0..s {
                        img.put(x * s + px, (sz - 1 - z) * s + py, [c.r, c.g, c.b, 255]);
                    }
                }
            }
        }
    }
    if o.outline {
        img.outline(0.55);
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(img: &Image, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * img.width + x) * 4) as usize;
        img.rgba[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn one_cube_shows_three_shades() {
        let mut v = Volume::new([1, 1, 1]);
        v.set(0, 0, 0, 1);
        let p = Palette::specification_default();
        let o = IsoOptions { views: 1, outline: false, ..Default::default() };
        let img = iso(&v, &p, &o);
        // A = 4, B = 2, H = 4: 8 wide, 8 tall.
        assert_eq!((img.width, img.height), (8, 8));
        let top = pixel(&img, 4, 1);
        let left = pixel(&img, 1, 5);
        let right = pixel(&img, 6, 5);
        assert!(top[3] == 255 && left[3] == 255 && right[3] == 255);
        assert!(top != left && left != right && top != right, "{top:?} {left:?} {right:?}");
        // The corners outside the cube stay clear.
        assert_eq!(pixel(&img, 0, 0)[3], 0);
    }

    #[test]
    fn four_views_side_by_side() {
        let mut v = Volume::new([3, 2, 2]);
        v.set(0, 0, 0, 1);
        let one = iso(&v, &Palette::specification_default(), &IsoOptions { views: 1, ..Default::default() });
        let four = iso(&v, &Palette::specification_default(), &IsoOptions::default());
        assert_eq!(four.width, one.width * 4);
    }

    #[test]
    fn flat_sprite_takes_the_front_voxel_and_casts_a_shadow() {
        let mut v = Volume::new([2, 3, 2]);
        v.set(0, 2, 0, 5); // behind
        v.set(0, 0, 0, 9); // in front
        let p = Palette::specification_default();
        let img = flat(&v, &p, &FlatOptions { size: 2, shadow: true, outline: false });
        assert_eq!((img.width, img.height), (6, 6));
        let c = p.color(9);
        // Bottom-left voxel: z = 0 is the lower row.
        assert_eq!(pixel(&img, 0, 2), [c.r, c.g, c.b, 255]);
        // Its shadow, down and to the right.
        assert_eq!(pixel(&img, 3, 5), [0, 0, 0, 90]);
        assert_eq!(pixel(&img, 5, 0)[3], 0);
    }
}
