//! The C headers in `orma_preload/include/orma_preload` are cbindgen's output for shm.rs and
//! events.rs (design 13.5). `ORMA_BLESS=1` rewrites them instead of comparing.

use std::path::{Path, PathBuf};

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn include_dir() -> PathBuf {
    crate_dir().join("../../collect/orma_preload/include/orma_preload")
}

/// Every struct of `src`: cbindgen otherwise writes only those exported functions use.
fn generate(config: &str, src: &str) -> String {
    let dir = crate_dir();
    let mut config = cbindgen::Config::from_file(dir.join(config)).unwrap();
    let text = std::fs::read_to_string(dir.join(src)).unwrap();
    config.export.include = text
        .lines()
        .filter_map(|l| l.strip_prefix("pub struct "))
        .map(|l| l.trim_end_matches(" {").to_string())
        .collect();
    let mut out = Vec::new();
    cbindgen::Builder::new()
        .with_config(config)
        .with_src(dir.join(src))
        .generate()
        .unwrap()
        .write(&mut out);
    String::from_utf8(out).unwrap()
}

fn check(config: &str, src: &str, header: &Path) {
    let generated = generate(config, src);
    if std::env::var_os("ORMA_BLESS").is_some() {
        std::fs::write(header, generated).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(header).unwrap_or_default();
    assert!(
        committed == generated,
        "{} is stale: run scripts/gen_headers.sh",
        header.display()
    );
}

#[test]
fn shm_header_is_generated() {
    check(
        "cbindgen_shm.toml",
        "src/shm.rs",
        &include_dir().join("orma_shm.h"),
    );
}

#[test]
fn events_header_is_generated() {
    check(
        "cbindgen_events.toml",
        "src/events.rs",
        &include_dir().join("orma_events.h"),
    );
}
