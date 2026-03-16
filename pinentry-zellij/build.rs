use std::path::PathBuf;
use std::process::Command;

use sha2::{Digest, Sha256};

fn main() {
    // Cargo builds the plugin for wasm32-wasip1 via artifact dependency
    // and provides the path through this env var.
    let artifact = std::env::var("CARGO_BIN_FILE_PINENTRY_ZELLIJ_PLUGIN_pinentry-zellij-plugin")
        .expect("CARGO_BIN_FILE_PINENTRY_ZELLIJ_PLUGIN_pinentry-zellij-plugin not set");

    // Copy to OUT_DIR so wasm-opt can modify it in place without touching
    // cargo's artifact output.
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let wasm_path = out_dir.join("pinentry-zellij-plugin.wasm");
    std::fs::copy(&artifact, &wasm_path).expect("failed to copy wasm artifact");

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
