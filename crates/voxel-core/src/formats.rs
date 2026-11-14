//! The other export formats, and the volume formats the editor imports.
//!
//! - **PLY**: ASCII, one quad per visible voxel face, each vertex with the
//!   face's colour; scaled by `scale` (0.1 by default), Z up.
//! - **Cubes**: the same header, every voxel a separate cube shrunk by
//!   `margin` (0.1).
//! - **Point cloud**: the same header with vertices only, one per voxel at
//!   its corner, scale 1.
//! - **XRAW**: `XRAW`, then the bytes 0 (unsigned), 4 (RGBA), 8 (bits per
//!   channel), 8 (bits per index), the size as three `u32`, 256 colours; the
//!   indices x fastest, then y, then z, 0 empty; the palette as 256 RGBA,
//!   entry `i` for index `i`.
//! - **Qubicle QB**: version 1.1.0.0, RGBA, right-handed, uncompressed, no
//!   visibility mask, one matrix; Y up, so the matrix is `(x, z, y)` in size
//!   and cell `(x, y, z)` lands at `(x, z, sy - 1 - y)`; each cell the RGBA
//!   of its colour.
//! - **SLAB6**: the size `(x, y, z)` as `u32`s, then one byte per cell, x
//!   slowest and z fastest with y and z reversed, holding the colour index
//!   minus one (255 empty); then 256 colours, 6 bits a channel, entry `k`
//!   for index `k + 1`.
//! - **Slices**: one image `x` wide and `y · z` tall, the z layers stacked,
//!   each pixel its voxel's colour, flipped vertically.
//!
//! Importers read XRAW, QB (compressed or not, RGBA or BGRA, either
//! handedness, every matrix) and SLAB6.

use crate::{Instance, Scene, Volume};
use glam::{IVec3, Vec3};
use std::fmt::Write;
use vox_format::{Color, Palette};

const PLY_HEADER: &str = "ply\nformat ascii 1.0\ncomment : Voxferro\n";

fn ply_vertex_props(out: &mut String) {
    out.push_str("property float x\nproperty float y\nproperty float z\n");
    out.push_str("property uchar red\nproperty uchar green\nproperty uchar blue\n");
}

/// Two tangents of a face, `u × v` along its normal, so `0, u, u + v, v`
/// runs counter-clockwise seen from outside.
fn tangents(face: crate::Face) -> (Vec3, Vec3) {
    use crate::Face::*;
    match face {
        PosX => (Vec3::Y, Vec3::Z),
        NegX => (Vec3::Z, Vec3::Y),
        PosY => (Vec3::Z, Vec3::X),
        NegY => (Vec3::X, Vec3::Z),
        PosZ => (Vec3::X, Vec3::Y),
        NegZ => (Vec3::Y, Vec3::X),
    }
}

/// The four corners of a cell's face, in cell units, counter-clockwise from
/// outside.
fn face_corners(cell: Vec3, face: crate::Face) -> [Vec3; 4] {
    let n = Vec3::from_array(face.normal());
    let base = cell + n.max(Vec3::ZERO);
    let (u, v) = tangents(face);
    [base, base + u, base + u + v, base + v]
}

/// The world positions of `corners` of a placed model, keeping their order
/// counter-clockwise from outside even under a mirroring transform.
fn place(instance: &Instance, pivot: Vec3, corners: [Vec3; 4], scale: f32) -> [Vec3; 4] {
    let mut out = corners.map(|c| instance.transform.apply(c - pivot) * scale);
    if instance.transform.rotation.determinant() < 0.0 {
        out.reverse();
    }
    out
}

fn colour_of(palette: &Palette, index: u8) -> Color {
    palette.color(index)
}

/// `PLY` of every visible face of the instances `keep` accepts.
pub fn ply_mesh(scene: &Scene, palette: &Palette, keep: impl Fn(&Instance) -> bool, scale: f32) -> String {
    let mut vertices = String::new();
    let mut faces = String::new();
    let mut count = 0usize;
    for instance in scene.instances.iter().filter(|i| keep(i)) {
        let Some(volume) = scene.models.get(instance.model_index) else { continue };
        let pivot = volume.pivot();
        let [sx, sy, sz] = volume.size();
        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    let v = volume.get(x, y, z);
                    if v == 0 {
                        continue;
                    }
                    let c = colour_of(palette, v);
                    let cell = IVec3::new(x as i32, y as i32, z as i32);
                    for face in crate::Face::ALL {
                        let o = face.offset();
                        let n = cell + IVec3::new(o[0], o[1], o[2]);
                        if volume.get_i(n.x, n.y, n.z) != 0 {
                            continue;
                        }
                        for p in place(instance, pivot, face_corners(cell.as_vec3(), face), scale) {
                            let _ = writeln!(vertices, "{} {} {} {} {} {}", p.x, p.y, p.z, c.r, c.g, c.b);
                        }
                        let k = count * 4;
                        let _ = writeln!(faces, "4 {} {} {} {}", k, k + 1, k + 2, k + 3);
                        count += 1;
                    }
                }
            }
        }
    }
    let mut out = String::from(PLY_HEADER);
    let _ = writeln!(out, "element vertex {}", count * 4);
    ply_vertex_props(&mut out);
    let _ = writeln!(out, "element face {count}\nproperty list uchar int vertex_index\nend_header");
    out + &vertices + &faces
}

/// `PLY` with every voxel a separate cube, shrunk by `margin` of a voxel.
pub fn ply_cubes(scene: &Scene, palette: &Palette, keep: impl Fn(&Instance) -> bool, scale: f32, margin: f32) -> String {
    let mut vertices = String::new();
    let mut faces = String::new();
    let mut cubes = 0usize;
    let shrink = margin.clamp(0.0, 1.0) * 0.5;
    for instance in scene.instances.iter().filter(|i| keep(i)) {
        let Some(volume) = scene.models.get(instance.model_index) else { continue };
        let pivot = volume.pivot();
        let [sx, sy, sz] = volume.size();
        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    let v = volume.get(x, y, z);
                    if v == 0 {
                        continue;
                    }
                    let c = colour_of(palette, v);
                    let cell = Vec3::new(x as f32, y as f32, z as f32);
                    for face in crate::Face::ALL {
                        let corners = face_corners(cell, face).map(|p| {
                            // Pull each corner toward the cell's centre.
                            let centre = cell + 0.5;
                            centre + (p - centre) * (1.0 - 2.0 * shrink)
                        });
                        for p in place(instance, pivot, corners, scale) {
                            let _ = writeln!(vertices, "{} {} {} {} {} {}", p.x, p.y, p.z, c.r, c.g, c.b);
                        }
                    }
                    for f in 0..6 {
                        let k = cubes * 24 + f * 4;
                        let _ = writeln!(faces, "4 {} {} {} {}", k, k + 1, k + 2, k + 3);
                    }
                    cubes += 1;
                }
            }
        }
    }
    let mut out = String::from(PLY_HEADER);
    let _ = writeln!(out, "element vertex {}", cubes * 24);
    ply_vertex_props(&mut out);
    let _ = writeln!(out, "element face {}\nproperty list uchar int vertex_index\nend_header", cubes * 6);
    out + &vertices + &faces
}

/// `PLY` points, one per voxel at its corner plus `offset` (0..1).
pub fn ply_points(scene: &Scene, palette: &Palette, keep: impl Fn(&Instance) -> bool, scale: f32, offset: Vec3) -> String {
    let mut vertices = String::new();
    let mut count = 0usize;
    for instance in scene.instances.iter().filter(|i| keep(i)) {
        let Some(volume) = scene.models.get(instance.model_index) else { continue };
        let pivot = volume.pivot();
        let [sx, sy, sz] = volume.size();
        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    let v = volume.get(x, y, z);
                    if v == 0 {
                        continue;
                    }
                    let c = colour_of(palette, v);
                    let p = instance.transform.apply(Vec3::new(x as f32, y as f32, z as f32) + offset - pivot) * scale;
                    let _ = writeln!(vertices, "{} {} {} {} {} {}", p.x, p.y, p.z, c.r, c.g, c.b);
                    count += 1;
                }
            }
        }
    }
    let mut out = String::from(PLY_HEADER);
    let _ = writeln!(out, "element vertex {count}");
    ply_vertex_props(&mut out);
    out.push_str("end_header\n");
    out + &vertices
}

fn u32le(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// An XRAW file of one model.
pub fn xraw(volume: &Volume, palette: &Palette) -> Vec<u8> {
    let [sx, sy, sz] = volume.size();
    let mut out = b"XRAW".to_vec();
    out.extend_from_slice(&[0, 4, 8, 8]);
    for s in [sx, sy, sz, 256] {
        u32le(&mut out, s);
    }
    for z in 0..sz {
        for y in 0..sy {
            for x in 0..sx {
                out.push(volume.get(x, y, z));
            }
        }
    }
    for c in palette.as_slice() {
        out.extend_from_slice(&[c.r, c.g, c.b, c.a]);
    }
    out
}

/// A model read from a file in another format, with its colours: either
/// palette indices with the file's palette, or RGBA per voxel to be matched.
#[derive(Debug, Clone)]
pub struct Imported {
    pub name: String,
    pub size: [u32; 3],
    /// Where the model's corner sits, in this editor's axes.
    pub position: IVec3,
    pub cells: Cells,
}

#[derive(Debug, Clone)]
pub enum Cells {
    /// Indices (0 empty), x fastest, then y, then z, and their palette.
    Indexed(Vec<u8>, Box<[Color; 256]>),
    /// Colours, in the same order; alpha 0 is empty.
    Rgba(Vec<[u8; 4]>),
}

impl Imported {
    /// The model in this palette: indexed colours mapped through their own
    /// palette, and every colour to the nearest one of `palette`.
    pub fn to_volume(&self, palette: &Palette) -> Volume {
        let [sx, sy, sz] = self.size;
        let mut volume = Volume::new(self.size);
        let mut cache = std::collections::HashMap::new();
        let mut nearest = |c: [u8; 3]| *cache.entry(c).or_insert_with(|| nearest_index(palette, c));
        let mut i = 0usize;
        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    let v = match &self.cells {
                        Cells::Indexed(idx, pal) => match idx[i] {
                            0 => 0,
                            k => {
                                let c = pal[k as usize];
                                nearest([c.r, c.g, c.b])
                            }
                        },
                        Cells::Rgba(rgba) => match rgba[i] {
                            [_, _, _, 0] => 0,
                            [r, g, b, _] => nearest([r, g, b]),
                        },
                    };
                    if v != 0 {
                        volume.set(x, y, z, v);
                    }
                    i += 1;
                }
            }
        }
        volume
    }
}

/// The index of `palette` nearest `c` in RGB (never 0).
pub fn nearest_index(palette: &Palette, c: [u8; 3]) -> u8 {
    (1..=255u8)
        .min_by_key(|&j| {
            let d = palette.color(j);
            let (r, g, b) = (c[0] as i32 - d.r as i32, c[1] as i32 - d.g as i32, c[2] as i32 - d.b as i32);
            r * r + g * g + b * b
        })
        .unwrap_or(1)
}

struct Bytes<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.data.len()).ok_or("the file ends early")?;
        let out = &self.data[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }

    fn i32(&mut self) -> Result<i32, String> {
        Ok(self.u32()? as i32)
    }
}

/// A size a model can have, or an error naming the problem.
fn check_size(size: [u32; 3]) -> Result<[u32; 3], String> {
    if size.iter().all(|&s| (1..=256).contains(&s)) {
        Ok(size)
    } else {
        Err(format!("a model of {}x{}x{} is outside 1..256", size[0], size[1], size[2]))
    }
}

pub fn read_xraw(data: &[u8]) -> Result<Imported, String> {
    let mut b = Bytes { data, at: 0 };
    if b.take(4)? != b"XRAW" {
        return Err("not an XRAW file".to_owned());
    }
    let (kind, channels, bits, index_bits) = (b.u8()?, b.u8()?, b.u8()?, b.u8()?);
    if kind != 0 || bits != 8 || !(1..=4).contains(&channels) {
        return Err(format!("XRAW with {bits}-bit channels of type {kind}: only 8-bit unsigned is read"));
    }
    let size = check_size([b.u32()?, b.u32()?, b.u32()?])?;
    let colours = b.u32()? as usize;
    let count = (size[0] * size[1] * size[2]) as usize;
    let rgba = |p: &[u8]| -> [u8; 4] {
        match channels {
            4 => [p[0], p[1], p[2], p[3]],
            3 => [p[0], p[1], p[2], 255],
            2 => [p[0], p[1], 0, 255],
            _ => [p[0], p[0], p[0], 255],
        }
    };
    let cells = match index_bits {
        8 => {
            let idx = b.take(count)?.to_vec();
            let mut pal = Box::new([Color::default(); 256]);
            for slot in pal.iter_mut().take(colours.min(256)) {
                let [r, g, b2, a] = rgba(b.take(channels as usize)?);
                *slot = Color { r, g, b: b2, a };
            }
            Cells::Indexed(idx, pal)
        }
        0 => {
            let raw = b.take(count * channels as usize)?;
            Cells::Rgba(raw.chunks(channels as usize).map(rgba).collect())
        }
        n => return Err(format!("XRAW with {n}-bit indices: only 8-bit or none is read")),
    };
    Ok(Imported { name: String::new(), size, position: IVec3::ZERO, cells })
}

/// A Qubicle Binary file of one model.
pub fn qb(volume: &Volume, palette: &Palette, name: &str) -> Vec<u8> {
    let [sx, sy, sz] = volume.size();
    let mut out = vec![1, 1, 0, 0];
    for v in [0, 1, 0, 0, 1] {
        u32le(&mut out, v);
    }
    let name: Vec<u8> = name.bytes().take(127).collect();
    out.push(name.len() as u8);
    out.extend_from_slice(&name);
    for s in [sx, sz, sy] {
        u32le(&mut out, s);
    }
    out.extend_from_slice(&[0; 12]);
    // Qubicle's cell (X, Y, Z) is this editor's (X, sy - 1 - Z, Y).
    for qz in 0..sy {
        for qy in 0..sz {
            for qx in 0..sx {
                let v = volume.get(qx, sy - 1 - qz, qy);
                let c = if v == 0 { [0; 4] } else { let c = palette.color(v); [c.r, c.g, c.b, 255] };
                out.extend_from_slice(&c);
            }
        }
    }
    out
}

pub fn read_qb(data: &[u8]) -> Result<Vec<Imported>, String> {
    let mut b = Bytes { data, at: 0 };
    let _version = b.u32()?;
    let format = b.u32()?;
    let right_handed = b.u32()? == 1;
    let compressed = b.u32()? == 1;
    let _mask = b.u32()?;
    let matrices = b.u32()?;
    if matrices > 4096 {
        return Err("not a Qubicle file".to_owned());
    }
    let colour = |v: u32| -> [u8; 4] {
        let [p0, p1, p2, a] = v.to_le_bytes();
        let a = if a == 0 { 0 } else { 255 };
        if format == 1 { [p2, p1, p0, a] } else { [p0, p1, p2, a] }
    };
    let mut out = Vec::new();
    for _ in 0..matrices {
        let len = b.u8()? as usize;
        let name = String::from_utf8_lossy(b.take(len)?).into_owned();
        let qsize = [b.u32()?, b.u32()?, b.u32()?];
        let qpos = [b.i32()?, b.i32()?, b.i32()?];
        let (qx, qy, qz) = (qsize[0] as usize, qsize[1] as usize, qsize[2] as usize);
        let qcount = qx.checked_mul(qy).and_then(|n| n.checked_mul(qz)).filter(|&n| n <= 1 << 26).ok_or("matrix too large")?;
        let mut grid = vec![[0u8; 4]; qcount];
        if compressed {
            for z in 0..qz {
                let mut i = 0usize;
                loop {
                    let v = b.u32()?;
                    if v == 6 {
                        break;
                    }
                    let (n, v) = if v == 2 { (b.u32()? as usize, b.u32()?) } else { (1, v) };
                    for _ in 0..n {
                        if i < qx * qy {
                            grid[z * qx * qy + i] = colour(v);
                        }
                        i += 1;
                    }
                }
            }
        } else {
            for cell in grid.iter_mut() {
                *cell = colour(b.u32()?);
            }
        }
        // Back to Z up: (X, Y, Z) -> (X, Z', Y), with Z' reversed when the
        // file is right-handed.
        let size = check_size([qsize[0], qsize[2], qsize[1]])?;
        let [sx, sy, sz] = size;
        let mut cells = vec![[0u8; 4]; (sx * sy * sz) as usize];
        for z in 0..qz {
            for y in 0..qy {
                for x in 0..qx {
                    let ty = if right_handed { qz - 1 - z } else { z };
                    let to = x + ty * sx as usize + y * (sx * sy) as usize;
                    cells[to] = grid[x + y * qx + z * qx * qy];
                }
            }
        }
        let position = if right_handed {
            IVec3::new(qpos[0], -(qpos[2] + qsize[2] as i32), qpos[1])
        } else {
            IVec3::new(qpos[0], qpos[2], qpos[1])
        };
        out.push(Imported { name, size, position, cells: Cells::Rgba(cells) });
    }
    Ok(out)
}

/// A SLAB6 `.vox` of one model.
pub fn slab(volume: &Volume, palette: &Palette) -> Vec<u8> {
    let [sx, sy, sz] = volume.size();
    let mut out = Vec::new();
    for s in [sx, sy, sz] {
        u32le(&mut out, s);
    }
    for x in 0..sx {
        for y in (0..sy).rev() {
            for z in (0..sz).rev() {
                out.push(volume.get(x, y, z).wrapping_sub(1));
            }
        }
    }
    for k in 0..256usize {
        let c = palette.color(((k + 1) % 256) as u8);
        out.extend_from_slice(&[c.r >> 2, c.g >> 2, c.b >> 2]);
    }
    out
}

pub fn read_slab(data: &[u8]) -> Result<Imported, String> {
    let mut b = Bytes { data, at: 0 };
    let size = check_size([b.u32()?, b.u32()?, b.u32()?])?;
    let [sx, sy, sz] = size;
    let raw = b.take((sx * sy * sz) as usize)?;
    let pal_bytes = b.take(768)?;
    let mut pal = Box::new([Color::default(); 256]);
    for k in 0..256usize {
        let p = &pal_bytes[k * 3..k * 3 + 3];
        // 6 bits a channel; spread back over 8.
        let up = |v: u8| (v.min(63) << 2) | (v.min(63) >> 4);
        pal[(k + 1) % 256] = Color { r: up(p[0]), g: up(p[1]), b: up(p[2]), a: 255 };
    }
    let mut idx = vec![0u8; raw.len()];
    let mut i = 0usize;
    for x in 0..sx {
        for y in (0..sy).rev() {
            for z in (0..sz).rev() {
                let v = raw[i].wrapping_add(1);
                idx[(x + y * sx + z * sx * sy) as usize] = v;
                i += 1;
            }
        }
    }
    Ok(Imported { name: String::new(), size, position: IVec3::ZERO, cells: Cells::Indexed(idx, pal) })
}

/// Whether `data` has exactly the length a SLAB6 file of its stated size
/// would: the format has no magic number to go by.
pub fn looks_like_slab(data: &[u8]) -> bool {
    if data.len() < 12 {
        return false;
    }
    let d = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().expect("4 bytes")) as u64;
    let (x, y, z) = (d(0), d(4), d(8));
    (1..=256).contains(&x) && (1..=256).contains(&y) && (1..=256).contains(&z) && data.len() as u64 == 12 + x * y * z + 768
}

/// The slices image: width, height and RGBA.
pub fn slices(volume: &Volume, palette: &Palette) -> (u32, u32, Vec<u8>) {
    let [sx, sy, sz] = volume.size();
    let height = sy * sz;
    let mut rgba = vec![0u8; (sx * height * 4) as usize];
    for z in 0..sz {
        for y in 0..sy {
            // Flipped vertically.
            let row = height - 1 - (z * sy + y);
            for x in 0..sx {
                let v = volume.get(x, y, z);
                if v != 0 {
                    let c = palette.color(v);
                    let at = ((row * sx + x) * 4) as usize;
                    rgba[at..at + 4].copy_from_slice(&[c.r, c.g, c.b, 255]);
                }
            }
        }
    }
    (sx, height, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Volume, Palette) {
        let mut v = Volume::new([3, 4, 5]);
        v.set(0, 0, 0, 1);
        v.set(2, 3, 4, 200);
        v.set(1, 2, 0, 7);
        (v, Palette::specification_default())
    }

    fn same(a: &Volume, b: &Volume) -> bool {
        let [sx, sy, sz] = a.size();
        a.size() == b.size()
            && (0..sz).all(|z| (0..sy).all(|y| (0..sx).all(|x| a.get(x, y, z) == b.get(x, y, z))))
    }

    #[test]
    fn xraw_round_trip_and_header() {
        let (v, p) = sample();
        let bytes = xraw(&v, &p);
        assert_eq!(&bytes[..8], b"XRAW\x00\x04\x08\x08");
        assert_eq!(bytes.len(), 24 + 60 + 1024);
        // x fastest: cell (1, 2, 0) is at 1 + 2 * 3.
        assert_eq!(bytes[24 + 7], 7);
        assert!(same(&read_xraw(&bytes).unwrap().to_volume(&p), &v));
    }

    #[test]
    fn qb_round_trip_and_axes() {
        let (v, p) = sample();
        let bytes = qb(&v, &p, "model");
        // Version, format, handedness, compression, mask, one matrix.
        assert_eq!(&bytes[..24], &[1, 1, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]);
        let m = read_qb(&bytes).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].name, "model");
        assert!(same(&m[0].to_volume(&p), &v));
        // Size as (x, z, y): 3, 5, 4.
        let at = 24 + 1 + 5;
        assert_eq!(&bytes[at..at + 12], &[3, 0, 0, 0, 5, 0, 0, 0, 4, 0, 0, 0]);
    }

    #[test]
    fn compressed_bgra_qb_reads() {
        // One 2x1x1 matrix, BGRA, left-handed, compressed: a run of two
        // red voxels, then the slice end.
        let mut d = vec![1, 1, 0, 0];
        for v in [1u32, 0, 1, 0, 1] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.push(1);
        d.push(b'm');
        for v in [2u32, 1, 1, 0, 0, 0, 2, 2] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&[0, 0, 255, 255]); // BGRA red
        d.extend_from_slice(&6u32.to_le_bytes());
        let m = read_qb(&d).unwrap();
        let Cells::Rgba(cells) = &m[0].cells else { panic!() };
        assert_eq!(cells, &vec![[255, 0, 0, 255]; 2]);
    }

    #[test]
    fn slab_round_trip() {
        let (v, p) = sample();
        let bytes = slab(&v, &p);
        assert!(looks_like_slab(&bytes));
        // Empty is 255; x slowest, z fastest, y and z reversed: the first
        // byte is cell (0, sy-1, sz-1).
        assert_eq!(bytes[12], 255);
        let back = read_slab(&bytes).unwrap();
        // 6-bit colours come back close enough to match the same indices.
        assert!(same(&back.to_volume(&p), &v));
    }

    #[test]
    fn slices_stack_layers_flipped() {
        let (v, p) = sample();
        let (w, h, rgba) = slices(&v, &p);
        assert_eq!((w, h), (3, 20));
        // Cell (0, 0, 0) is the bottom row's first pixel.
        let c = p.color(1);
        assert_eq!(&rgba[((h - 1) * w * 4) as usize..][..4], &[c.r, c.g, c.b, 255]);
    }

    #[test]
    fn ply_counts() {
        let mut scene = Scene::default();
        let (v, p) = sample();
        scene.models.push(v);
        scene.instances.push(Instance { model_index: 0, ..Default::default() });
        let mesh = ply_mesh(&scene, &p, |_| true, 0.1);
        assert!(mesh.starts_with("ply\nformat ascii 1.0\n"));
        assert!(mesh.contains("element vertex 72\n") && mesh.contains("element face 18\n"));
        let cubes = ply_cubes(&scene, &p, |_| true, 0.1, 0.1);
        assert!(cubes.contains("element vertex 72\n") && cubes.contains("element face 18\n"));
        let points = ply_points(&scene, &p, |_| true, 1.0, Vec3::ZERO);
        assert!(points.contains("element vertex 3\n") && !points.contains("element face"));
        // Three header lines, the element, six properties, end_header, three points.
        assert_eq!(points.lines().count(), 3 + 1 + 6 + 1 + 3);
    }

    #[test]
    fn faces_wind_outward() {
        for face in crate::Face::ALL {
            let [a, b, _, d] = face_corners(Vec3::ZERO, face);
            let n = (b - a).cross(d - a);
            assert_eq!(n, Vec3::from_array(face.normal()), "{face:?}");
        }
    }
}
