//! Exporting a scene as Wavefront OBJ.
//!
//! The same three files the editor's own `io_obj` writes: the geometry, a
//! material library, and the palette as a 256x1 texture that every face
//! samples by its palette index. Faces are merged greedily, so a flat wall is
//! two triangles however many voxels it spans.
//!
//! Coordinates: one unit per voxel, and Y up, as most tools importing OBJ
//! expect; the editor's Z-up world is turned with (x, y, z) -> (x, z, -y).

use crate::{Instance, Scene};
use glam::Vec3;
use std::fmt::Write;
use vox_format::Palette;

/// The three files of an export.
pub struct ObjExport {
    pub obj: String,
    pub mtl: String,
    pub png: Vec<u8>,
    pub quads: usize,
}

/// Exports the instances `keep` accepts. `stem` names the `.mtl` and `.png`
/// the `.obj` refers to, so the three must be written side by side as
/// `stem.obj`, `stem.mtl` and `stem.png`.
pub fn export_obj(scene: &Scene, palette: &Palette, keep: impl Fn(&Instance) -> bool, stem: &str) -> ObjExport {
    let mut obj = String::new();
    let _ = writeln!(obj, "# Voxferro export\nmtllib {stem}.mtl\no {stem}\nusemtl palette");

    // One texture coordinate per palette index, at the centre of its texel.
    for i in 0..256 {
        let _ = writeln!(obj, "vt {:.6} 0.5", (i as f32 + 0.5) / 256.0);
    }
    // Normals in the order of `Face::ALL`, turned to Y up.
    for face in crate::Face::ALL {
        let n = y_up(Vec3::from_array(face.normal()));
        let _ = writeln!(obj, "vn {} {} {}", n.x, n.y, n.z);
    }

    let mut vertices = 0usize;
    let mut quads = 0usize;
    for instance in scene.instances.iter().filter(|i| keep(i)) {
        let Some(volume) = scene.models.get(instance.model_index) else { continue };
        let pivot = volume.pivot();
        for quad in crate::greedy::greedy_quads(volume, |x, y, z| volume.get(x, y, z) as u16) {
            for corner in quad.corners {
                let p = y_up(instance.transform.apply(corner - pivot));
                let _ = writeln!(obj, "v {} {} {}", p.x, p.y, p.z);
            }
            // The normal after the instance's rotation, as an index into the
            // six written above.
            let world = instance.transform.rotation * Vec3::from_array(quad.face.normal());
            let normal = crate::Face::ALL
                .iter()
                .position(|f| Vec3::from_array(f.normal()).dot(world) > 0.5)
                .unwrap_or(0)
                + 1;
            let uv = quad.value as usize + 1;
            let v = vertices + 1;
            let _ = writeln!(
                obj,
                "f {}/{uv}/{normal} {}/{uv}/{normal} {}/{uv}/{normal} {}/{uv}/{normal}",
                v,
                v + 1,
                v + 2,
                v + 3
            );
            vertices += 4;
            quads += 1;
        }
    }

    let mtl = format!(
        "# Voxferro export\nnewmtl palette\nKa 0 0 0\nKd 1 1 1\nKs 0 0 0\nillum 1\nmap_Kd {stem}.png\n"
    );
    let mut rgba = Vec::with_capacity(256 * 4);
    for c in palette.as_slice() {
        rgba.extend_from_slice(&[c.r, c.g, c.b, 255]);
    }
    ObjExport { obj, mtl, png: png_rgba(256, 1, &rgba), quads }
}

fn y_up(p: Vec3) -> Vec3 {
    Vec3::new(p.x, p.z, -p.y)
}

/// A minimal PNG encoder: RGBA, no filtering, zlib "stored" blocks.
///
/// Uncompressed is fine for a 256x1 palette (about a kilobyte) and needs no
/// dependency; every reader accepts stored deflate blocks.
pub fn png_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    // Each row starts with its filter type, 0 (none).
    let row = width as usize * 4;
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * row..(y + 1) * row]);
    }

    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 bits, RGBA, deflate, no filter, no interlace

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [(b"IHDR", &ihdr), (b"IDAT", &zlib), (b"IEND", &Vec::new())] {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Transform, Volume};

    fn scene() -> Scene {
        let mut v = Volume::new([2, 2, 2]);
        for z in 0..2 {
            for y in 0..2 {
                for x in 0..2 {
                    v.set(x, y, z, 5);
                }
            }
        }
        Scene {
            models: vec![v],
            instances: vec![Instance {
                model_index: 0,
                transform: Transform::from_parts(0b0000_0100, [0, 0, 1]),
                layer: -1,
                ..Default::default()
            }],
            nodes: Vec::new(),
            frame: 0,
        }
    }

    #[test]
    fn a_cube_exports_as_six_quads_on_the_ground() {
        let e = export_obj(&scene(), &Palette::default(), |_| true, "cube");
        assert_eq!(e.quads, 6);
        assert!(e.obj.contains("mtllib cube.mtl") && e.mtl.contains("map_Kd cube.png"));

        let ys: Vec<f32> = e
            .obj
            .lines()
            .filter_map(|l| l.strip_prefix("v "))
            .map(|l| l.split(' ').nth(1).unwrap().parse().unwrap())
            .collect();
        assert_eq!(ys.len(), 24);
        assert_eq!(ys.iter().cloned().fold(f32::INFINITY, f32::min), 0.0, "Y up, resting on y = 0");

        // Every face indexes the texel of palette index 5: vt line 6.
        assert!(e.obj.lines().filter(|l| l.starts_with("f ")).all(|l| l.contains("/6/")));
    }

    #[test]
    fn the_png_checksums_are_right() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082, "the well-known IEND CRC");
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        let png = png_rgba(2, 1, &[255, 0, 0, 255, 0, 255, 0, 255]);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(png.ends_with(&[0xAE, 0x42, 0x60, 0x82]));
    }
}
