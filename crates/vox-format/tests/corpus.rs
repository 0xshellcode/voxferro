//! Checks the parser against the real the reference editor sample models.
//!
//! The corpus is not committed (see `assets/README.md`), so these tests skip
//! themselves when it is missing rather than failing a clean checkout.

use std::path::{Path, PathBuf};

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/vox")
}

/// Every `.vox` under the corpus, including `anim/`.
fn corpus_files() -> Vec<PathBuf> {
    fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|e| e == "vox") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    collect(&corpus_dir(), &mut out);
    out.sort();
    out
}

macro_rules! skip_if_no_corpus {
    ($files:expr) => {
        if $files.is_empty() {
            eprintln!(
                "skipping: no corpus at {} (see assets/README.md)",
                corpus_dir().display()
            );
            return;
        }
    };
}

#[test]
fn every_sample_has_a_supported_version() {
    let files = corpus_files();
    skip_if_no_corpus!(files);

    // The corpus spans both format versions on purpose: the macOS a recent version
    // build writes 150, the win64 a recent version build writes 200. Keeping both in
    // the corpus is what stops a 150-only assumption from creeping back in.
    let mut by_version: std::collections::BTreeMap<u32, usize> = Default::default();

    for path in &files {
        let bytes = std::fs::read(path).expect("corpus file should be readable");
        let version = vox_format::peek_version(&bytes)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(
            vox_format::is_supported_version(version),
            "{} reports unsupported version {version}",
            path.display()
        );
        *by_version.entry(version).or_default() += 1;
    }

    eprintln!("validated {} files: {:?}", files.len(), by_version);
    assert!(
        by_version.len() >= 2,
        "corpus should cover both format versions, found {by_version:?}"
    );
}

#[test]
fn every_sample_parses_fully() {
    let files = corpus_files();
    skip_if_no_corpus!(files);

    let mut unknown: std::collections::BTreeSet<String> = Default::default();

    for path in &files {
        let bytes = std::fs::read(path).unwrap();
        let file = vox_format::VoxFile::parse(&bytes)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));

        assert!(
            !file.models.is_empty(),
            "{} parsed without a single model",
            path.display()
        );

        // Every voxel must sit inside its model's declared box, and never
        // carry index 0 — that value means "empty" and is not stored.
        for (m, model) in file.models.iter().enumerate() {
            for v in &model.voxels {
                assert!(
                    (v.x as u32) < model.size[0]
                        && (v.y as u32) < model.size[1]
                        && (v.z as u32) < model.size[2],
                    "{}: model {m} has voxel {:?} outside size {:?}",
                    path.display(),
                    (v.x, v.y, v.z),
                    model.size
                );
                assert_ne!(v.index, 0, "{}: stored voxel with index 0", path.display());
            }
        }

        // Shape nodes must reference models that exist.
        for node in &file.nodes {
            if let vox_format::SceneNode::Shape(shape) = node {
                for m in &shape.models {
                    assert!(
                        (m.model_id as usize) < file.models.len(),
                        "{}: shape {} references missing model {}",
                        path.display(),
                        shape.id,
                        m.model_id
                    );
                }
            }
        }

        unknown.extend(file.unknown_chunks.iter().map(|c| c.id.clone()));
    }

    // Not an assertion: a new chunk id showing up is information, not a
    // failure. Printing it keeps the gap visible.
    eprintln!("unknown chunk ids across corpus: {unknown:?}");
}

#[test]
fn matches_dot_vox_on_the_whole_corpus() {
    let files = corpus_files();
    skip_if_no_corpus!(files);

    for path in &files {
        let bytes = std::fs::read(path).unwrap();
        let ours = vox_format::VoxFile::parse(&bytes).unwrap();
        let theirs = dot_vox::load_bytes(&bytes)
            .unwrap_or_else(|e| panic!("{}: dot_vox failed: {e}", path.display()));

        assert_eq!(
            ours.version,
            theirs.version,
            "{}: version disagrees",
            path.display()
        );
        assert_eq!(
            ours.models.len(),
            theirs.models.len(),
            "{}: model count disagrees",
            path.display()
        );

        for (i, (a, b)) in ours.models.iter().zip(&theirs.models).enumerate() {
            assert_eq!(
                a.size,
                [b.size.x, b.size.y, b.size.z],
                "{}: model {i} size disagrees",
                path.display()
            );
            assert_eq!(
                a.voxels.len(),
                b.voxels.len(),
                "{}: model {i} voxel count disagrees",
                path.display()
            );

            for (ours_v, theirs_v) in a.voxels.iter().zip(&b.voxels) {
                assert_eq!(
                    (ours_v.x, ours_v.y, ours_v.z),
                    (theirs_v.x, theirs_v.y, theirs_v.z),
                    "{}: model {i} voxel position disagrees",
                    path.display()
                );

                // dot_vox normalises the index to 0-based; we keep the file's
                // 1-based value and let Palette::color hide the shift. Both
                // must resolve to the same colour.
                assert_eq!(
                    ours_v.index,
                    theirs_v.i.wrapping_add(1),
                    "{}: model {i} voxel index disagrees",
                    path.display()
                );

                let ours_c = ours.palette.color(ours_v.index);
                let theirs_c = theirs.palette[theirs_v.i as usize];
                assert_eq!(
                    (ours_c.r, ours_c.g, ours_c.b, ours_c.a),
                    (theirs_c.r, theirs_c.g, theirs_c.b, theirs_c.a),
                    "{}: colour for voxel index {} disagrees",
                    path.display(),
                    ours_v.index
                );
            }
        }
    }

    eprintln!("cross-checked {} files against dot_vox", files.len());
}

#[test]
fn parses_the_smallest_sample_exactly() {
    let path = corpus_dir().join("3x3x3.vox");
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("skipping: {} not present", path.display());
        return;
    };

    let file = vox_format::VoxFile::parse(&bytes).unwrap();

    assert_eq!(file.version, vox_format::VERSION_150);
    assert_eq!(file.models.len(), 1);
    assert_eq!(file.models[0].size, [3, 3, 3]);
    assert_eq!(file.models[0].voxels.len(), 20);
    assert_eq!(file.layers.len(), 8, "the editor always writes 8 layers");
    assert_eq!(
        file.materials.len(),
        256,
        "one material per palette index"
    );
    assert!(
        file.cameras.is_empty(),
        "rCAM arrived in version 200"
    );
    // IMAP is optional even in version 150: the editor only writes it once
    // the palette has been reordered. Across the corpus exactly one file has
    // one, so asserting its presence here would be wrong.
    assert!(file.index_map.is_none());

    // The scene is the minimal shape: transform -> group -> transform -> shape.
    assert_eq!(file.nodes.len(), 4);
}

#[test]
fn reads_version_200_additions() {
    let path = corpus_dir().join("anim/Gomez.vox");
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("skipping: {} not present", path.display());
        return;
    };

    let file = vox_format::VoxFile::parse(&bytes).unwrap();
    assert_eq!(file.version, vox_format::VERSION_200);

    // META carries the animation range on animated models.
    let range = file
        .meta
        .iter()
        .find_map(|d| d.get("_anim_range"))
        .expect("animated model should declare _anim_range");
    assert_eq!(range, "0 30");

    assert!(
        file.index_map.is_none(),
        "version 200 dropped IMAP"
    );
}

#[test]
fn reads_the_optional_index_map() {
    // room.vox is the only sample with a reordered palette, so it is the only
    // one carrying IMAP. It is the regression guard for that chunk.
    let path = corpus_dir().join("room.vox");
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("skipping: {} not present", path.display());
        return;
    };

    let file = vox_format::VoxFile::parse(&bytes).unwrap();
    let map = file.index_map.expect("room.vox carries an IMAP chunk");
    assert_eq!(map.len(), 256);
    // The map is a permutation of palette slots, so no entry may repeat.
    let unique: std::collections::BTreeSet<u8> = map.iter().copied().collect();
    assert_eq!(unique.len(), 256, "IMAP should be a permutation");
}
