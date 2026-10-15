//! Writes a test scene exercising every material kind.
//!
//! The sample corpus barely uses materials -- one file has emitters, none has
//! glass -- so the path tracer needs a scene built to cover them. It is
//! written as a real `.vox`, which doubles as a check that the reference editor itself
//! reads materials we write.
//!
//!     cargo run -p voxel-core --example materials_demo -- out.vox

use glam::IVec3;
use vox_format::{Color, Dict, Material, Palette, VoxFile};
use voxel_core::Volume;

const FLOOR: u8 = 1;
const DIFFUSE: u8 = 2;
const METAL: u8 = 3;
const GLASS: u8 = 4;
const EMIT: u8 = 5;
const ROUGH_METAL: u8 = 6;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "materials_demo.vox".to_owned());

    let size = [112u32, 48, 40];
    let mut volume = Volume::new(size);

    // Floor slab, two voxels thick.
    for x in 0..size[0] {
        for y in 0..size[1] {
            for z in 0..2 {
                volume.set(x, y, z, FLOOR);
            }
        }
    }

    // A row of spheres, one per material.
    let radius = 8.5f32;
    for (i, index) in [DIFFUSE, METAL, ROUGH_METAL, GLASS].into_iter().enumerate() {
        let centre = IVec3::new(14 + i as i32 * 22, 24, 11);
        fill_sphere(&mut volume, centre, radius, index);
    }

    // An emissive block at the end, so its light falls on the others.
    for x in 96..106 {
        for y in 19..29 {
            for z in 2..20 {
                volume.set(x, y, z, EMIT);
            }
        }
    }

    let mut entries = [Color { r: 0, g: 0, b: 0, a: 0 }; 256];
    // Chunk entry i is the colour of voxel index i + 1.
    let mut set = |index: u8, r: u8, g: u8, b: u8| {
        entries[(index - 1) as usize] = Color { r, g, b, a: 255 };
    };
    set(FLOOR, 180, 180, 180);
    set(DIFFUSE, 200, 60, 50);
    set(METAL, 230, 190, 110);
    set(ROUGH_METAL, 170, 175, 185);
    set(GLASS, 170, 220, 255);
    set(EMIT, 255, 210, 160);

    // Version 150 conventions throughout: every key present, and `_weight`
    // carrying the metal amount / transparency / emission per type.
    let material = |id: u8, pairs: &[(&str, &str)]| Material {
        id: id as i32,
        properties: Dict(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()),
    };
    let materials = vec![
        material(FLOOR, &[("_type", "_diffuse"), ("_weight", "1"), ("_rough", "0.1"), ("_ior", "0.3")]),
        material(DIFFUSE, &[("_type", "_diffuse"), ("_weight", "1"), ("_rough", "0.1"), ("_ior", "0.3")]),
        material(METAL, &[("_type", "_metal"), ("_weight", "1"), ("_rough", "0.05"), ("_ior", "0.3")]),
        material(ROUGH_METAL, &[("_type", "_metal"), ("_weight", "1"), ("_rough", "0.4"), ("_ior", "0.3")]),
        material(GLASS, &[("_type", "_glass"), ("_weight", "0.9"), ("_rough", "0.0"), ("_ior", "0.5")]),
        material(EMIT, &[("_type", "_emit"), ("_weight", "1"), ("_flux", "2"), ("_rough", "0.1"), ("_ior", "0.3")]),
    ];

    let file = VoxFile {
        version: vox_format::VERSION_150,
        models: vec![volume.to_model()],
        palette: Palette::from_chunk(&entries),
        nodes: Vec::new(), // the writer adds a minimal graph
        layers: Vec::new(),
        materials,
        render_objects: Vec::new(),
        cameras: Vec::new(),
        color_names: Vec::new(),
        index_map: None,
        meta: Vec::new(),
        unknown_chunks: Vec::new(),
    };

    std::fs::write(&path, file.to_bytes()).expect("write demo scene");
    eprintln!("wrote {path}: {} voxels", volume.filled_count());
}

fn fill_sphere(volume: &mut Volume, centre: IVec3, radius: f32, index: u8) {
    let r = radius.ceil() as i32;
    for z in -r..=r {
        for y in -r..=r {
            for x in -r..=r {
                let p = IVec3::new(x, y, z);
                if p.as_vec3().length() <= radius {
                    let q = centre + p;
                    if q.min_element() >= 0 {
                        volume.set(q.x as u32, q.y as u32, q.z as u32, index);
                    }
                }
            }
        }
    }
}
