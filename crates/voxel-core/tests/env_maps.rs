//! Parses the HDR maps in `assets/ibl`. Skips when they are not present
//! (see assets/README.md).

use std::path::Path;
use voxel_core::{EnvMap, EnvSampling};

#[test]
fn every_map_in_the_assets_parses() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ibl");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("skipping: no maps at {}", dir.display());
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "hdr") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let map = EnvMap::parse_hdr(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(map.pixels.len(), (map.width * map.height) as usize);
        assert!(
            map.pixels.iter().flatten().all(|v| v.is_finite() && *v >= 0.0),
            "{} has invalid texels",
            path.display()
        );
        let s = EnvSampling::new(&map);
        assert!(s.integral > 0.0 && s.total > 0.0);
        eprintln!(
            "{}: {}x{}, integral {:.3}, scale(1) {:.3}",
            path.file_name().unwrap().to_string_lossy(),
            map.width,
            map.height,
            s.integral,
            s.scale(1.0)
        );
    }
}
