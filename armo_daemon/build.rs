use aya_build::{Package, Toolchain};

// aya-ebpf needs nightly for -Z build-std=core. Only the eBPF crate is built with it; armod stays
// on the stable in rust-toolchain.toml.
const EBPF_TOOLCHAIN: &str = "nightly-2026-09-28";

fn main() -> aya_build::Result<()> {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../armo_ebpf");
    aya_build::build_ebpf(
        [Package {
            name: "armo-ebpf",
            root_dir: root,
            ..Default::default()
        }],
        Toolchain::Custom(EBPF_TOOLCHAIN),
    )
}
