//! Times the editor's CPU operations on a large model:
//!
//!     cargo run --release -p voxel-core --example perf [size]
//!
//! The model is a cube `size` voxels on a side (256 by default) holding a
//! noisy ball of several colours -- a hard, realistic case: lots of surface,
//! nothing uniform.
use glam::{IVec3, Vec3};
use std::time::Instant;
use voxel_core::{Command, Document, History, Instance, Scene, Volume, brush, formats, kernels, marching, sprites, voxelize};
use vox_format::Palette;

fn model(n: u32) -> Volume {
    let mut v = Volume::new([n; 3]);
    let c = n as f32 / 2.0;
    let mut seed = 1u32;
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let p = Vec3::new(x as f32, y as f32, z as f32) - c;
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let noise = (seed >> 16 & 7) as f32 - 3.5;
                if p.length() + noise * 0.6 < c * 0.85 {
                    v.set(x, y, z, 1 + (seed >> 20 & 7) as u8);
                }
            }
        }
    }
    v
}

fn time<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let out = f();
    println!("{name:<28} {:>9.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    out
}

fn main() {
    let n: u32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(256);
    let v = time(&format!("build {n}^3 model"), || model(n));
    println!("{} voxels filled\n", v.filled_count());

    println!("-- Viewport mesh");
    for occlusion in [false, true] {
        let label = if occlusion { "with occlusion" } else { "plain" };
        let mut scene = voxel_core::Scene::default();
        scene.models.push(v.clone());
        scene.instances.push(voxel_core::Instance::default());
        let mut cache = voxel_core::mesh_cache::MeshCache::default();
        cache.occlusion = occlusion;
        time(&format!("  full, {label}"), || cache.build(&scene, |_| true, |_| None, |_| false));
        scene.models[0].set(n / 2 + 5, n / 2 + 5, n / 2 + 5, 3);
        time(&format!("  one voxel, {label}"), || cache.build(&scene, |_| true, |_| None, |_| false));
    }

    let palette = Palette::specification_default();
    let six = kernels::neighbours(6).unwrap();
    let all = kernels::neighbours(26).unwrap();

    println!("-- Edit panel");
    time("full", || kernels::full(&v, 3));
    time("reverse", || kernels::reverse(&v, 3));
    time("recolour (fill)", || kernels::recolour(&v, 3));
    time("loop x 5", || kernels::shift_wrap(&v, 0, 5));
    time("scale z 0.5", || kernels::scale(&v, Vec3::new(1.0, 1.0, 0.5)));
    time("halve (1/2)", || kernels::halve(&v));
    time("shear z 0.2 0.2", || kernels::shear(&v, 2, [0.2, 0.2]));
    time("rotate z 30", || kernels::rotate_by(&v, 2, 30.0));
    time("repeat x 1 (same size)", || kernels::repeat(&v, [1, 1, 1]));
    time("mirror x", || kernels::symmetrize(&v, 0));
    time("diagonal z", || kernels::diagonal(&v, 2));
    time("dilate 6", || kernels::dilate(&v, 3, &six));
    time("dilate 26", || kernels::dilate(&v, 3, &all));
    time("erode 6", || kernels::erode(&v, &six));
    time("prim ellipsoid", || kernels::prim(&v, kernels::Prim::Ellipsoid, 3));
    time("hull", || kernels::hull(&v));
    time("flood", || kernels::flood(&v, 3));

    println!("\n-- Brushes and selection");
    let mid = IVec3::splat(n as i32 / 2);
    let surface = (0..n as i32).map(|x| IVec3::new(x, mid.y, mid.z)).find(|p| v.get_i(p.x, p.y, p.z) != 0).unwrap();
    time("region same colour", || brush::region(&v, surface, brush::Region::SameColour).len());
    time("region connected geo 8", || brush::region(&v, surface, brush::Region::Connected { by_colour: false, wide: true }).len());
    let opts = brush::BrushOptions { size: 64, sphere: true, ..Default::default() };
    time("voxel tip 64 sphere", || brush::tip_cells(mid, 2, 0, &opts).len());

    println!("\n-- Exports");
    let mut scene = Scene::default();
    scene.models.push(v.clone());
    scene.instances.push(Instance { model_index: 0, ..Default::default() });
    let ply = time("ply (faces)", || formats::ply_mesh(&scene, &palette, |_| true, 0.1));
    println!("{:<28} {:>9.1} MB", "  ply size", ply.len() as f64 / 1e6);
    time("ply (points)", || formats::ply_points(&scene, &palette, |_| true, 1.0, Vec3::ZERO).len());
    let tris = time("marching cubes (triangles)", || marching::triangles(&scene, |_| true).len());
    println!("{:<28} {:>9}", "  triangles", tris);
    time("xraw", || formats::xraw(&v, &palette).len());
    time("qb", || formats::qb(&v, &palette, "m").len());
    time("slab", || formats::slab(&v, &palette).len());
    time("iso sprite (4 views)", || sprites::iso(&v, &palette, &Default::default()).width);
    time("2d sprite", || sprites::flat(&v, &palette, &Default::default()).width);

    println!("\n-- Imports");
    let qb = formats::qb(&v, &palette, "m");
    let back = time("read qb", || formats::read_qb(&qb).unwrap());
    time("qb to volume (matching)", || back[0].to_volume(&palette).filled_count());
    // A sphere mesh of 20k triangles, voxelised at 256.
    let mut mesh = voxelize::Mesh::default();
    let steps = 100;
    let point = |i: u32, j: u32| {
        let (th, ph) = (i as f32 / steps as f32 * std::f32::consts::PI, j as f32 / steps as f32 * std::f32::consts::TAU);
        (Vec3::new(th.sin() * ph.cos(), th.sin() * ph.sin(), th.cos()), [200u8, 120, 60])
    };
    for i in 0..steps {
        for j in 0..steps {
            let (a, b, c, d) = (point(i, j), point(i + 1, j), point(i + 1, j + 1), point(i, j + 1));
            mesh.triangles.push([a, b, c]);
            mesh.triangles.push([a, c, d]);
        }
    }
    time("voxelize 20k tris at 256", || voxelize::voxelize(&mesh, &palette, 256, false).filled_count());
    time("voxelize + fill", || voxelize::voxelize(&mesh, &palette, 256, true).filled_count());

    // An edit of the whole model as the app records it: every changed cell
    // a write, one undo step.
    println!("\n-- Undo history");
    let mut doc = Document { scene: scene.clone(), ..Default::default() };
    let after = kernels::reverse(&v, 3);
    let command = time("diff (changed cells)", || Command::diff(&doc, 0, &after, |_| true));
    if let Command::SetVoxels { changes, .. } = &command {
        println!("{:<28} {:>9}", "  changes", changes.len());
    }
    let mut history = History::default();
    time("apply and record", || history.exec_alone(&mut doc, command));
    time("undo", || history.undo(&mut doc));
    time("redo", || history.redo(&mut doc));
}
