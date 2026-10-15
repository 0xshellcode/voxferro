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

#[test]
fn every_sample_has_a_supported_version() {
    let files = corpus_files();
    if files.is_empty() {
        eprintln!(
            "skipping: no corpus at {} (see assets/README.md)",
            corpus_dir().display()
        );
        return;
    }

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
