//! Mutation fuzzing of everything a `.vox` file goes through when opened:
//!
//!     cargo run --release -p voxel-core --example fuzz_vox -- [iterations] [seed] [seed files...]
//!
//! Takes seed files (by default the showcase scenes and whatever is in
//! `assets/vox`), damages copies of them -- bits flipped, counts and sizes
//! set to awkward values, chunks cut short, repeated or spliced -- and runs
//! each through the editor's whole load path: parsing (and recovering),
//! the scene graph at a few frames, the mesh, the path tracer's grid and
//! writing the file back. A panic, a run over five seconds, or a crash is a
//! failure: the input is kept in `target/fuzz/seed-<n>/` to reproduce it with
//! `fuzz_vox --one <file>`.
//!
//! `FUZZ_FORMAT=qb|xraw|slab|obj|hdr` fuzzes the other files a user can
//! open instead -- Qubicle, XRAW and SLAB6 imports (read, then turned into
//! a model in the palette), OBJ voxelising, HDR environment maps -- from
//! seeds written by the editor's own exporters.
//!
//! Plain Rust and stable: no `cargo fuzz`, no nightly. Each case's bytes
//! are written to `target/fuzz/seed-<n>/current.vox` before it runs, so even an
//! abort (an allocation the system refuses) leaves the input behind.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use vox_format::VoxFile;
use voxel_core::kernels::Rand;
use voxel_core::{Scene, mesh_cache::MeshCache};

fn format() -> String {
    std::env::var("FUZZ_FORMAT").unwrap_or_else(|_| "vox".to_owned())
}

/// Runs one input of another format through its import.
fn import_path(format: &str, bytes: &[u8]) {
    use voxel_core::{EnvMap, formats, voxelize};
    let palette = vox_format::Palette::specification_default();
    let models = match format {
        "qb" => formats::read_qb(bytes).unwrap_or_default(),
        "xraw" => formats::read_xraw(bytes).into_iter().collect(),
        "slab" => formats::read_slab(bytes).into_iter().collect(),
        "obj" => {
            let text = String::from_utf8_lossy(bytes);
            let materials = voxelize::parse_mtl(&text);
            if let Ok(mesh) = voxelize::parse_obj(&text, &materials) {
                for (size, fill) in [(16, false), (64, true)] {
                    std::hint::black_box(voxelize::voxelize(&mesh, &palette, size, fill).filled_count());
                }
            }
            Vec::new()
        }
        "hdr" => {
            if let Ok(map) = EnvMap::parse_hdr(bytes) {
                std::hint::black_box(map.width);
            }
            Vec::new()
        }
        other => panic!("unknown FUZZ_FORMAT {other}"),
    };
    for m in models {
        std::hint::black_box(m.to_volume(&palette).filled_count());
    }
}

/// Runs one input through the load path. Errors are fine; panics are not.
fn load_path(bytes: &[u8]) {
    let format = format();
    if format != "vox" {
        return import_path(&format, bytes);
    }
    // FUZZ_VERBOSE=1 says how far a case gets, and how long each step takes.
    let verbose = std::env::var_os("FUZZ_VERBOSE").is_some();
    let t = Instant::now();
    let step = |what: &str| {
        if verbose {
            eprintln!("{:>10.1?}  {what}", t.elapsed());
        }
    };
    let files = [VoxFile::parse(bytes).ok(), VoxFile::recover(bytes)];
    step("parsed");
    for file in files.into_iter().flatten() {
        // What else the editor reads from it: the materials, the palette's
        // display order, the saved cameras.
        for m in &file.materials {
            std::hint::black_box(m.resolve());
        }
        std::hint::black_box(voxel_core::display_order(file.index_map.as_deref()));
        std::hint::black_box(voxel_core::saved_camera::slots(&file.cameras));
        if file.models.is_empty() {
            continue;
        }
        if verbose {
            let sizes: Vec<_> = file.models.iter().map(|m| m.size).collect();
            eprintln!("  models {sizes:?}, {} nodes", file.nodes.len());
        }
        for frame in [0, 1, 7] {
            let scene = Scene::from_file_at_frame(&file, frame);
            step(&format!("scene at frame {frame}: {} instances", scene.instances.len()));
            let mut cache = MeshCache::default();
            let mesh = cache.build(&scene, |_| true, |_| None);
            std::hint::black_box(mesh.triangle_count());
            step("meshed");
            let _ = scene.bake(|_| true, 64 * 1024 * 1024);
            step("baked");
            // Global Time off inside a group: the graph walked at two frames.
            let mut held = scene.clone();
            held.held = file.nodes.iter().skip(1).find_map(|n| match n {
                vox_format::SceneNode::Transform(t) => Some((t.id, frame + 3)),
                _ => None,
            });
            held.rebuild();
            std::hint::black_box(held.instances.len());
        }
        let again = file.to_bytes();
        // What the editor writes, it must read back.
        if let Err(e) = VoxFile::parse(&again) {
            panic!("wrote a file it cannot read: {e}");
        }
    }
}

const INTERESTING: [u32; 12] = [0, 1, 2, 3, 127, 255, 256, 257, 4096, 65536, 0x7FFF_FFFF, 0xFFFF_FFFF];

fn mutate(seed: &[u8], rand: &mut Rand, corpus: &[Vec<u8>]) -> Vec<u8> {
    let mut out = seed.to_vec();
    let below = |rand: &mut Rand, n: usize| (rand.next_u32() as usize) % n.max(1);
    for _ in 0..1 + below(rand, 6) {
        if out.is_empty() {
            out.extend_from_slice(b"VOX ");
        }
        let at = below(rand, out.len());
        match below(rand, 9) {
            0 => out[at] ^= 1 << below(rand, 8),
            1 => out[at] = rand.next_u32() as u8,
            // A 32-bit field set to an awkward value: counts, sizes, ids.
            2 | 3 => {
                let v = INTERESTING[below(rand, INTERESTING.len())].to_le_bytes();
                let at = at & !3;
                for (k, b) in v.iter().enumerate() {
                    if let Some(slot) = out.get_mut(at + k) {
                        *slot = *b;
                    }
                }
            }
            4 => out.truncate(at),
            5 => {
                let len = below(rand, 64).min(out.len() - at);
                out.drain(at..at + len);
            }
            6 => {
                let len = below(rand, 256).min(out.len() - at);
                let piece = out[at..at + len].to_vec();
                let to = below(rand, out.len());
                out.splice(to..to, piece);
            }
            7 => {
                let n = below(rand, 16);
                for _ in 0..n {
                    out.insert(at, rand.next_u32() as u8);
                }
            }
            // A piece of another file spliced in.
            _ => {
                let other = &corpus[below(rand, corpus.len())];
                let from = below(rand, other.len());
                let len = below(rand, 512).min(other.len() - from);
                let to = below(rand, out.len());
                out.splice(to..to, other[from..from + len].iter().copied());
            }
        }
    }
    out
}

fn seeds(args: &[String]) -> Vec<Vec<u8>> {
    let mut paths: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
    if paths.is_empty() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for dir in ["assets/showcase", "assets/vox"] {
            if let Ok(entries) = std::fs::read_dir(root.join(dir)) {
                for e in entries.flatten() {
                    let p = e.path();
                    // Small files mutate faster; large ones add little.
                    if p.extension().is_some_and(|x| x == "vox") && e.metadata().is_ok_and(|m| m.len() < 200_000) {
                        paths.push(p);
                    }
                }
            }
        }
        paths.push(root.join("assets/materials_demo.vox"));
    }
    paths.sort();
    let mut out: Vec<Vec<u8>> = paths.iter().filter_map(|p| std::fs::read(p).ok()).collect();
    let format = format();
    if format != "vox" {
        return other_seeds(&format, &out);
    }
    // And a few made here: one voxel, an empty model, a two-model scene.
    let mut tiny = VoxFile::new_single([1, 1, 1]);
    tiny.models[0].voxels.push(vox_format::Voxel { x: 0, y: 0, z: 0, index: 1 });
    out.push(tiny.to_bytes());
    out.push(VoxFile::new_single([4, 4, 4]).to_bytes());
    out.push(newer_chunks_seed());
    out
}

/// A file with what the readers learnt last: a palette order (`IMAP`), a
/// Blend material, a camera with a roll, and a looping animation.
fn newer_chunks_seed() -> Vec<u8> {
    use vox_format::{Camera, Dict, Material, SceneNode};
    let mut file = VoxFile::new_single([3, 3, 3]);
    file.models[0].voxels.push(vox_format::Voxel { x: 1, y: 1, z: 0, index: 7 });
    file.index_map = Some(Box::new(std::array::from_fn(|k| (255 - k) as u8)));
    let dict = |pairs: &[(&str, &str)]| Dict(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
    file.materials.push(Material { id: 7, properties: dict(&[("_type", "_blend"), ("_metal", "0.3"), ("_trans", "0.2"), ("_emit", "0.5")]) });
    file.cameras.push(Camera {
        id: 0,
        attributes: dict(&[("_mode", "pers"), ("_focus", "0 0 0"), ("_angle", "30 45 12"), ("_radius", "40"), ("_frustum", "0.4"), ("_fov", "45")]),
    });
    for node in &mut file.nodes {
        if let SceneNode::Transform(t) = node
            && t.id != 0
        {
            t.attributes.set("_loop", "1");
            t.frames = vec![dict(&[("_f", "0"), ("_t", "0 0 0")]), dict(&[("_f", "4"), ("_t", "5 0 2"), ("_r", "20")])];
        }
    }
    let bytes = file.to_bytes();
    let back = VoxFile::parse(&bytes).expect("the seed reads back");
    assert!(back.index_map.is_some() && !back.cameras.is_empty() && !back.materials.is_empty());
    bytes
}

/// Seeds for the other formats, written from the `.vox` seeds' models.
fn other_seeds(format: &str, vox: &[Vec<u8>]) -> Vec<Vec<u8>> {
    use voxel_core::{Volume, formats};
    let palette = vox_format::Palette::specification_default();
    let volumes: Vec<Volume> = vox
        .iter()
        .filter_map(|b| VoxFile::parse(b).ok())
        .flat_map(|f| f.models.into_iter().take(2))
        .filter(|m| m.size.iter().all(|&s| s <= 64))
        .map(|m| Volume::from_model(&m))
        .collect();
    match format {
        "qb" => volumes.iter().map(|v| formats::qb(v, &palette, "m")).collect(),
        "xraw" => volumes.iter().map(|v| formats::xraw(v, &palette)).collect(),
        "slab" => volumes.iter().map(|v| formats::slab(v, &palette)).collect(),
        "obj" => vec![
            b"mtllib a.mtl\nnewmtl red\nKd 1 0 0\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
              vt 0.25 0.5\nusemtl red\nf 1/1 2/1 3/1 4/1\nf 5 8 7 6\nf 1 5 6 2\nf 2 6 7 3\nf 3 7 8 4\nf 4 8 5 1\n"
                .to_vec(),
            b"v 0 0 0 1 0 0\nv 10 0 0 0 1 0\nv 0 3 7 0 0 1\nf -3 -2 -1\n".to_vec(),
        ],
        "hdr" => {
            let mut flat = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 4 +X 8\n".to_vec();
            flat.extend((0..32).flat_map(|i| [i as u8 * 7, 128, 64, 130]));
            // New-style run-length rows, as most maps are written.
            let mut rle = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 8\n".to_vec();
            for _ in 0..2 {
                rle.extend([2, 2, 0, 8]);
                for _ in 0..4 {
                    rle.extend([128 + 8, 100]);
                }
            }
            vec![flat, rle]
        }
        _ => Vec::new(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fuzz");
    std::fs::create_dir_all(&dir).expect("target/fuzz");
    if args.first().map(String::as_str) == Some("--one") {
        let bytes = std::fs::read(&args[1]).expect("the input");
        load_path(&bytes);
        println!("ok");
        return;
    }
    let iterations: u64 = args.first().and_then(|a| a.parse().ok()).unwrap_or(20_000);
    let seed: u64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(1);
    // One folder a seed, so runs side by side keep their inputs apart.
    let dir = dir.join(format!("{}-seed-{seed}", format()));
    std::fs::create_dir_all(&dir).expect("target/fuzz/seed-N");
    let corpus = seeds(args.get(2..).unwrap_or(&[]));
    println!("{} seeds ({}), {iterations} cases, seed {seed}", corpus.len(), format());

    // A watchdog: a case that runs too long is a hang.
    static STARTED: AtomicU64 = AtomicU64::new(0);
    static CASE: AtomicU64 = AtomicU64::new(0);
    let clock = Instant::now();
    let current = dir.join("current.vox");
    {
        let current = current.clone();
        let dir = dir.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let started = STARTED.load(Ordering::Relaxed);
                if started != 0 && (clock.elapsed().as_millis() as u64).saturating_sub(started) > std::env::var("FUZZ_HANG_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(5_000) {
                    let keep = dir.join("hang.vox");
                    let _ = std::fs::copy(&current, &keep);
                    eprintln!("HANG: case {} ran over 5 s; input kept as {}", CASE.load(Ordering::Relaxed), keep.display());
                    std::process::exit(2);
                }
            }
        });
    }
    std::panic::set_hook(Box::new(|_| {}));
    let mut rand = Rand::new(seed);
    let mut failures = 0;
    let start = Instant::now();
    for i in 0..iterations {
        let base = &corpus[(rand.next_u32() as usize) % corpus.len()];
        let input = mutate(base, &mut rand, &corpus);
        let _ = std::fs::write(&current, &input);
        CASE.store(i, Ordering::Relaxed);
        STARTED.store(clock.elapsed().as_millis() as u64 + 1, Ordering::Relaxed);
        let result = std::panic::catch_unwind(|| load_path(&input));
        STARTED.store(0, Ordering::Relaxed);
        if let Err(e) = result {
            failures += 1;
            let what = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
            let keep = dir.join(format!("panic-{i}.vox"));
            let _ = std::fs::write(&keep, &input);
            eprintln!("PANIC in case {i}: {what}\n  input kept as {}", keep.display());
            if failures >= 10 {
                break;
            }
        }
    }
    let _ = std::fs::remove_file(&current);
    println!("{failures} failures in {:.1?}", start.elapsed());
    if failures > 0 {
        std::process::exit(1);
    }
}
