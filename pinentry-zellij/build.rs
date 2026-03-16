use std::path::PathBuf;
use std::process::Command;

use sha2::{Digest, Sha256};

fn main() {
    // Rebuild when plugin or protocol source changes
    println!("cargo:rerun-if-changed=../pinentry-zellij-plugin/src/");
    println!("cargo:rerun-if-changed=../pinentry-zellij-plugin/Cargo.toml");
    println!("cargo:rerun-if-changed=../pinentry-zellij-plugin/.cargo/config.toml");
    println!("cargo:rerun-if-changed=../pinentry-zellij-protocol/src/");
    println!("cargo:rerun-if-changed=../pinentry-zellij-protocol/Cargo.toml");

    // Use a separate target-dir to avoid Cargo lock contention with the
    // outer build (both builds run concurrently under the workspace lock).
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let wasm_target_dir = PathBuf::from(&out_dir).join("wasm-target");

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let plugin_manifest = PathBuf::from(&manifest_dir)
        .parent()
        .expect("cannot find workspace root")
        .join("pinentry-zellij-plugin")
        .join("Cargo.toml");

    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-wasip1",
            "--target-dir",
        ])
        .arg(&wasm_target_dir)
        .arg("--manifest-path")
        .arg(&plugin_manifest)
        .arg("-q")
        // Prevent the outer cargo's flags/wrappers from leaking into the
        // wasm build. CARGO_ENCODED_RUSTFLAGS carries host link args (e.g.
        // -fuse-ld=mold), RUSTC_WRAPPER may point to coverage/sccache
        // wrappers that inject flags incompatible with wasm.
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .status()
        .expect("failed to run cargo build for wasm plugin");

    assert!(status.success(), "wasm plugin build failed");

    let wasm_path = wasm_target_dir
        .join("wasm32-wasip1")
        .join("release")
        .join("pinentry-zellij-plugin.wasm");

    assert!(
        wasm_path.exists(),
        "wasm not found at {}",
        wasm_path.display()
    );

    // Optimize wasm with wasm-opt in release builds only.
    // Flags tuned for wasmi (interpreter, no SIMD, no threads).
    if std::env::var("PROFILE").unwrap() == "release" {
        if let Ok(status) = Command::new("wasm-opt")
            .args([
                "-O3",
                "--zero-filled-memory",
                "--traps-never-happen",
                "--low-memory-unused",
                "--enable-bulk-memory",
                "--enable-sign-ext",
                "--enable-mutable-globals",
                "--enable-nontrapping-float-to-int",
                "--enable-multivalue",
                "--enable-reference-types",
                "--enable-extended-const",
                "-o",
            ])
            .arg(&wasm_path)
            .arg(&wasm_path)
            .status()
        {
            if !status.success() {
                println!("cargo:warning=wasm-opt failed (exit {status}), using unoptimized wasm");
            }
        } else {
            println!("cargo:warning=wasm-opt not found, using unoptimized wasm");
        }
    }

    // Compute SHA-256 hash of the (possibly optimized) wasm binary
    let wasm_bytes = std::fs::read(&wasm_path).expect("failed to read wasm file");
    let hash = Sha256::digest(&wasm_bytes);
    // First 16 bytes (32 hex chars) — sufficient for version detection
    let hash_hex = hash
        .iter()
        .take(16)
        .fold(String::with_capacity(32), |mut acc, b| {
            use std::fmt::Write;
            let _ = write!(acc, "{b:02x}");
            acc
        });

    // Emit env vars for the source code to use
    println!("cargo:rustc-env=PLUGIN_WASM_PATH={}", wasm_path.display());
    println!("cargo:rustc-env=PLUGIN_WASM_HASH={hash_hex}");
}
