//! Build script for ghostty_vt_sys
//!
//! This script:
//! 1. Locates the Zig compiler
//! 2. Builds libghostty-vt using Zig
//! 3. Links the resulting static library

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

mod build_support;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Find workspace root (two levels up from crates/ghostty_vt_sys)
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("Could not find workspace root");

    // Check for vendored Ghostty
    let ghostty_dir = workspace_root.join("vendor/ghostty");
    let build_zig_zon = ghostty_dir.join("build.zig.zon");

    if !ghostty_dir.exists() {
        panic!(
            "\n\nGhostty submodule not found at {}\n\n\
             This is required to build ghostty_vt_sys.\n\n\
             To fix, run:\n\
             \n\
             git submodule update --init --recursive\n\n",
            ghostty_dir.display()
        );
    }

    // Check if submodule is initialized (directory exists but is empty/not populated)
    if !build_zig_zon.exists() {
        panic!(
            "\n\nGhostty submodule exists but is not initialized at {}\n\n\
             The vendor/ghostty directory exists but appears to be empty.\n\
             This commonly happens in git worktrees where submodules\n\
             were not properly initialized.\n\n\
             To fix, run:\n\
             \n\
             git submodule update --init --recursive\n\n",
            ghostty_dir.display()
        );
    }

    // Find Zig compiler
    let zig = find_zig(workspace_root).expect(
        "Zig compiler not found. Install with: ./scripts/bootstrap-zig.sh\n\
         Or set ZIG environment variable to point to zig binary.",
    );

    // Rebuild triggers
    println!("cargo:rerun-if-changed=zig/build.zig");
    println!("cargo:rerun-if-changed=zig/build.zig.zon");
    println!("cargo:rerun-if-changed=zig/lib.zig");
    println!("cargo:rerun-if-changed=build_support.rs");
    println!("cargo:rerun-if-changed=include/ghostty_vt.h");
    println!(
        "cargo:rerun-if-changed={}",
        ghostty_dir.join("build.zig.zon").display()
    );

    // Build with Zig
    let zig_out = out_dir.join("zig-out");
    let mut cmd = Command::new(&zig);
    cmd.current_dir(manifest_dir.join("zig"))
        .arg("build")
        .arg("-Doptimize=ReleaseFast")
        .arg("--prefix")
        .arg(&zig_out);

    // Always pass an explicit Zig target, including for native builds. Leaving
    // Zig's target unspecified enables every instruction supported by the build
    // host. A release runner with AVX-512 can otherwise produce a CAS binary
    // that SIGILLs on another supported x86_64 machine inside Ghostty.
    let target = env::var("TARGET").expect(
        "Cargo did not provide TARGET; refusing to build ghostty_vt_sys without an explicit portable Zig target",
    );
    let zig_target =
        build_support::rust_target_to_zig(&target).unwrap_or_else(|message| panic!("{message}"));
    cmd.arg(format!("-Dtarget={zig_target}"));

    let status = cmd.status().expect("Failed to execute zig build");

    if !status.success() {
        panic!("Zig build failed with status: {status}");
    }

    // Link the static library
    let lib_dir = zig_out.join("lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=ghostty_vt");

    // Link C standard library
    println!("cargo:rustc-link-lib=c");
}

/// Find the Zig compiler
fn find_zig(workspace_root: &Path) -> Option<PathBuf> {
    // 1. Check ZIG environment variable
    if let Ok(zig) = env::var("ZIG") {
        let path = PathBuf::from(zig);
        if path.exists() {
            return Some(path);
        }
    }

    // 2. Check system PATH first (for mise, homebrew, etc.)
    if let Ok(output) = Command::new("which").arg("zig").output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }

    // 3. Check workspace .context/zig/zig (bootstrapped location) as fallback
    let bootstrapped = workspace_root.join(".context/zig/zig");
    if bootstrapped.exists() {
        return Some(bootstrapped);
    }

    None
}
