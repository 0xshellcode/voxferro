//! Importing a mesh: a Wavefront OBJ turned into voxels, as the reference editor's
//! `the reference renderer` does.
//!
//! Read: `v` (with the common `v x y z r g b` vertex colours), `vt`, `f`
//! (polygons, fanned; negative indices count from the end), `usemtl`, and
//! from the `.mtl` the file names each material's `Kd` colour and `map_Kd`
//! texture -- which a palette-textured OBJ, the reference editor's or this editor's,
//! is coloured by: a corner with a texture coordinate takes the texel there. OBJ is usually Y up, so a
//! point comes in as `(x, -z, y)` -- the inverse of the editor's own OBJ
//! export. The mesh is scaled so its longest side spans `size` voxels, and
//! each triangle is sampled finely enough that no cell it crosses is
//! missed; with `fill`, the space the surface encloses is filled too.
//! Colours go to the nearest of the palette.

use crate::{Volume, kernels};
use glam::Vec3;
use std::collections::HashMap;
use vox_format::Palette;

/// A mesh read from OBJ: triangles with an RGB colour each corner.
#[derive(Debug, Default)]
pub struct Mesh {
    pub triangles: Vec<[(Vec3, [u8; 3]); 3]>,
}

/// A material of an `.mtl` file.
#[derive(Debug, Clone, Default)]
pub struct Material {
    pub kd: Option<[u8; 3]>,
    /// The `map_Kd` file name, and once the caller has read it, the image.
    pub texture_name: Option<String>,
    pub texture: Option<Texture>,
}

/// An RGBA image.
#[derive(Debug, Clone)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Texture {
    /// The texel at `(u, v)`, v up as OBJ has it, wrapping.
    fn sample(&self, uv: glam::Vec2) -> [u8; 3] {
        let x = ((uv.x.rem_euclid(1.0) * self.width as f32) as u32).min(self.width - 1);
        let y = (((1.0 - uv.y).rem_euclid(1.0) * self.height as f32) as u32).min(self.height - 1);
        let at = ((y * self.width + x) * 4) as usize;
        [self.rgba[at], self.rgba[at + 1], self.rgba[at + 2]]
    }
}

/// The materials of an `.mtl` file, by name.
pub fn parse_mtl(text: &str) -> HashMap<String, Material> {
    let mut out: HashMap<String, Material> = HashMap::new();
    let mut current = None;
    for line in text.lines() {
        let mut w = line.split_whitespace();
        match w.next() {
            Some("newmtl") => {
                current = w.next().map(str::to_owned);
                if let Some(name) = &current {
                    out.entry(name.clone()).or_default();
                }
            }
            Some("Kd") => {
                let rgb: Vec<f32> = w.filter_map(|n| n.parse().ok()).collect();
                if let (Some(name), [r, g, b, ..]) = (&current, rgb.as_slice()) {
                    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                    out.entry(name.clone()).or_default().kd = Some([c(*r), c(*g), c(*b)]);
                }
            }
            Some("map_Kd") => {
                if let (Some(name), Some(file)) = (&current, w.last()) {
                    out.entry(name.clone()).or_default().texture_name = Some(file.to_owned());
                }
            }
            _ => {}
        }
    }
    out
}

/// The `mtllib` an OBJ names, if any.
pub fn mtllib(obj: &str) -> Option<String> {
    obj.lines().find_map(|l| l.trim().strip_prefix("mtllib ").map(|s| s.trim().to_owned()))
}

/// The OBJ's triangles. `materials` colours faces by `usemtl`; faces without
/// one, and vertices without colours, are white.
pub fn parse_obj(text: &str, materials: &HashMap<String, Material>) -> Result<Mesh, String> {
    let mut points: Vec<(Vec3, Option<[u8; 3]>)> = Vec::new();
    let mut uvs: Vec<glam::Vec2> = Vec::new();
    let mut material: Option<&Material> = None;
    let mut mesh = Mesh::default();
    for (n, line) in text.lines().enumerate() {
        let mut w = line.split_whitespace();
        match w.next() {
            Some("v") => {
                let f: Vec<f32> = w.filter_map(|v| v.parse().ok()).collect();
                if f.len() < 3 {
                    return Err(format!("line {}: a vertex needs x y z", n + 1));
                }
                // Y up to Z up.
                let p = Vec3::new(f[0], -f[2], f[1]);
                let rgb = (f.len() >= 6).then(|| {
                    let c = |v: f32| if v > 1.0 { v.min(255.0) as u8 } else { (v.max(0.0) * 255.0).round() as u8 };
                    [c(f[3]), c(f[4]), c(f[5])]
                });
                points.push((p, rgb));
            }
            Some("vt") => {
                let f: Vec<f32> = w.filter_map(|v| v.parse().ok()).collect();
                uvs.push(glam::Vec2::new(*f.first().unwrap_or(&0.0), *f.get(1).unwrap_or(&0.0)));
            }
            Some("usemtl") => material = w.next().and_then(|m| materials.get(m)),
            Some("f") => {
                let mut corners = Vec::new();
                for item in w {
                    let mut parts = item.split('/');
                    let index: i64 = parts.next().and_then(|i| i.parse().ok()).ok_or_else(|| format!("line {}: bad face", n + 1))?;
                    let i = if index < 0 { points.len() as i64 + index } else { index - 1 };
                    let (p, rgb) = *points.get(usize::try_from(i).map_err(|_| format!("line {}: bad index", n + 1))?).ok_or_else(|| format!("line {}: index {index} out of range", n + 1))?;
                    // The texel under the corner's texture coordinate, if
                    // there is both a coordinate and a texture.
                    let texel = parts
                        .next()
                        .and_then(|t| t.parse::<i64>().ok())
                        .and_then(|t| uvs.get(usize::try_from(if t < 0 { uvs.len() as i64 + t } else { t - 1 }).ok()?))
                        .and_then(|uv| Some(material?.texture.as_ref()?.sample(*uv)));
                    let kd = material.and_then(|m| m.kd);
                    corners.push((p, rgb.or(texel).or(kd).unwrap_or([255, 255, 255])));
                }
                for k in 1..corners.len().saturating_sub(1) {
                    mesh.triangles.push([corners[0], corners[k], corners[k + 1]]);
                }
            }
            _ => {}
        }
    }
    if mesh.triangles.is_empty() {
        return Err("no faces".to_owned());
    }
    Ok(mesh)
}

/// The mesh as voxels, its longest side `size` voxels long (1 to 256).
pub fn voxelize(mesh: &Mesh, palette: &Palette, size: u32, fill: bool) -> Volume {
    let size = size.clamp(1, 256) as f32;
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for t in &mesh.triangles {
        for (p, _) in t {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
    }
    let extent = (hi - lo).max_element().max(1e-6);
    // Just under `size`, so the far side lands in the last cell.
    let scale = (size - 1e-3) / extent;
    let dims = ((hi - lo) * scale).floor().as_uvec3() + glam::UVec3::ONE;
    let dims = dims.min(glam::UVec3::splat(256)).to_array();
    let mut volume = Volume::new(dims);
    let mut nearest = HashMap::new();
    let mut colour_of = |c: [u8; 3]| *nearest.entry(c).or_insert_with(|| crate::formats::nearest_index(palette, c));
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|(p, _)| (p - lo) * scale);
        // Steps of under half a voxel along both sides cover every cell.
        let n = ((b - a).length().max((c - a).length()).max((c - b).length()) * 2.0).ceil().max(1.0) as u32;
        for i in 0..=n {
            for j in 0..=(n - i) {
                let (u, v) = (i as f32 / n as f32, j as f32 / n as f32);
                let p = a + (b - a) * u + (c - a) * v;
                let cell = p.floor().as_ivec3().clamp(glam::IVec3::ZERO, glam::IVec3::from_array(dims.map(|d| d as i32 - 1)));
                // The nearest corner's colour.
                let w = [1.0 - u - v, u, v];
                let k = (0..3).max_by(|&x, &y| w[x].total_cmp(&w[y])).unwrap_or(0);
                let index = colour_of(t[k].1);
                volume.set(cell.x as u32, cell.y as u32, cell.z as u32, index);
            }
        }
    }
    if fill {
        // Enclosed space takes the colour of the voxel below it, or the
        // first colour found, so the inside reads as the outside's.
        let any = (0..dims[2]).flat_map(|z| (0..dims[1]).flat_map(move |y| (0..dims[0]).map(move |x| (x, y, z)))).map(|(x, y, z)| volume.get(x, y, z)).find(|&v| v != 0);
        if let Some(c) = any {
            volume = kernels::flood(&volume, c);
        }
    }
    volume
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE: &str = "mtllib cube.mtl\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\nusemtl red\n\
        f 1 2 3 4\nf 5 8 7 6\nf 1 5 6 2\nf 2 6 7 3\nf 3 7 8 4\nf 4 8 5 1\n";

    #[test]
    fn a_cube_becomes_a_hollow_or_filled_box() {
        let materials = parse_mtl("newmtl red\nKd 1 0 0\nmap_Kd red.png\n");
        assert_eq!(materials["red"].texture_name.as_deref(), Some("red.png"));
        assert_eq!(mtllib(CUBE).as_deref(), Some("cube.mtl"));
        let mesh = parse_obj(CUBE, &materials).unwrap();
        assert_eq!(mesh.triangles.len(), 12);
        let mut palette = Palette::specification_default();
        palette.set_color(9, vox_format::Color { r: 255, g: 0, b: 0, a: 255 });
        let hollow = voxelize(&mesh, &palette, 8, false);
        assert_eq!(hollow.size(), [8, 8, 8]);
        assert_eq!(hollow.get(0, 0, 0), 9);
        assert_eq!(hollow.get(4, 4, 4), 0);
        assert_eq!(hollow.filled_count(), 8 * 8 * 8 - 6 * 6 * 6);
        let solid = voxelize(&mesh, &palette, 8, true);
        assert_eq!(solid.filled_count(), 512);
    }

    #[test]
    fn y_up_becomes_z_up_and_errors_are_named() {
        let tall = "v 0 0 0\nv 1 0 0\nv 0 4 0\nf 1 2 3\n";
        let v = voxelize(&parse_obj(tall, &HashMap::new()).unwrap(), &Palette::specification_default(), 8, false);
        assert_eq!(v.size()[2], 8, "OBJ's y is height");
        assert!(parse_obj("v 0 0 0\nf 1 2 9\n", &HashMap::new()).unwrap_err().contains("out of range"));
        assert!(parse_obj("v 0 0 0\n", &HashMap::new()).is_err());
    }

    #[test]
    fn a_textured_face_takes_its_texel() {
        let mut materials = parse_mtl("newmtl palette\nmap_Kd p.png\n");
        // Two texels: left red, right blue.
        materials.get_mut("palette").unwrap().texture =
            Some(Texture { width: 2, height: 1, rgba: vec![255, 0, 0, 255, 0, 0, 255, 255] });
        let obj = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0.75 0.5\nusemtl palette\nf 1/1 2/1 3/1\n";
        let mesh = parse_obj(obj, &materials).unwrap();
        assert_eq!(mesh.triangles[0][0].1, [0, 0, 255]);
    }
}
