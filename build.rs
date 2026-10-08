//! Compiles the CUDA kernel and hands the PTX to [`crate::gpu::cuda`].
//!
//! The kernel is a crate of its own because it targets `nvptx64-nvidia-cuda`
//! and uses a nightly ABI; it is built here rather than committed so that the
//! PTX in a binary is always the PTX its sources describe. A kernel changed
//! without the compiled form being refreshed is a defect that shows up as
//! wrong addresses and nothing else, so nothing is left to remember.
//!
//! `cuda-kernel/rust-toolchain.toml` names the compiler. Building from a clean
//! checkout therefore needs `rustup`, which fetches it.

use std::path::{Path, PathBuf};
use std::process::Command;

const KERNEL_TARGET: &str = "nvptx64-nvidia-cuda";

fn main() {
    let manifest = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    let kernel = manifest.join("cuda-kernel");
    let out = PathBuf::from(env("OUT_DIR"));

    for watched in ["src", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml"] {
        println!("cargo::rerun-if-changed=cuda-kernel/{watched}");
    }

    let wanted = out.join("cuda-kernel.ptx");

    // A container build compiles the kernel in a layer of its own, so that
    // editing the host code does not rebuild it. That layer hands the PTX over
    // through this variable; everywhere else it is unset and the kernel is
    // compiled below.
    println!("cargo::rerun-if-env-changed=ONION_GEN_KERNEL_PTX");
    if let Some(ready) = std::env::var_os("ONION_GEN_KERNEL_PTX") {
        let ready = PathBuf::from(ready);
        println!("cargo::rerun-if-changed={}", ready.display());
        if let Err(why) = copy_kernel(&ready, &wanted) {
            fail(&[&why, "ONION_GEN_KERNEL_PTX names the file that was used."]);
        }
        return;
    }

    let target_dir = out.join("kernel");
    let status = cargo_for_kernel(&kernel)
        .args(["build", "--release", "--target", KERNEL_TARGET])
        .arg("--target-dir")
        .arg(&target_dir)
        .status();

    match status {
        Ok(status) if status.success() => {}
        Ok(status) => fail(&[
            &format!("compiling the kernel failed: {status}"),
            NEEDS_RUSTUP,
        ]),
        Err(e) => fail(&[
            &format!("could not run cargo for the kernel: {e}"),
            NEEDS_RUSTUP,
        ]),
    }

    let built = target_dir
        .join(KERNEL_TARGET)
        .join("release")
        .join("ogkernel.ptx");
    if let Err(why) = copy_kernel(&built, &wanted) {
        fail(&[&why, NEEDS_RUSTUP]);
    }
}

/// Puts the PTX where `include_str!` will look for it.
///
/// An empty one would compile and then fail on the first card, which is a long
/// way from here, so it is refused at the only point that can still say why.
fn copy_kernel(from: &Path, to: &Path) -> Result<(), String> {
    match std::fs::copy(from, to) {
        Ok(0) => Err(format!("{} is empty", from.display())),
        Ok(_) => Ok(()),
        Err(e) => Err(format!("{}: {e}", from.display())),
    }
}

const NEEDS_RUSTUP: &str =
    "the CUDA kernel is compiled during this build and needs rustup on PATH. \
     See cuda-kernel/README.md.";

/// A `cargo` that obeys the kernel's own toolchain file rather than the one
/// building this crate.
///
/// The variables removed below are how the parent build describes itself —
/// which compiler, which target, which flags. Inherited, they would aim the
/// kernel at the host's architecture and pin it to stable.
fn cargo_for_kernel(dir: &Path) -> Command {
    // The rustup shim, not `$CARGO`: `$CARGO` is a particular toolchain's
    // binary and reads no toolchain file.
    let mut cmd = Command::new("cargo");
    cmd.current_dir(dir);
    for (key, _) in std::env::vars() {
        if key.starts_with("CARGO") || key.starts_with("RUST") {
            cmd.env_remove(key);
        }
    }
    for key in ["TARGET", "HOST", "CC", "CXX", "AR", "CFLAGS", "LDFLAGS"] {
        cmd.env_remove(key);
    }
    cmd
}

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("cargo sets {key}"))
}

fn fail(lines: &[&str]) -> ! {
    for line in lines {
        println!("cargo::error={line}");
    }
    std::process::exit(1);
}
