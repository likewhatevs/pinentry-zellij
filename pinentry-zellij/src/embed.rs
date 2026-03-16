//! Embedded wasm plugin self-installation.
//!
//! The plugin wasm is compiled into the binary at build time. On startup,
//! this module writes it to the Zellij plugin directory if the installed
//! version is missing or stale, using an xattr for version detection.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;
use tracing::{debug, warn};

/// Errors that can occur during plugin installation.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("config directory not found")]
    NoConfigDir,
    #[error("target path has no parent directory")]
    NoParentDir,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("failed to persist tempfile: {0}")]
    Persist(#[from] tempfile::PersistError),
}

/// The compiled wasm plugin, embedded at build time.
const PLUGIN_BYTES: &[u8] = include_bytes!(env!("PLUGIN_WASM_PATH"));

/// SHA-256 prefix of the embedded wasm, computed at build time.
const PLUGIN_HASH: &str = env!("PLUGIN_WASM_HASH");

/// Extended attribute name used for version detection.
const XATTR_NAME: &str = "user.pinentry-zellij.hash";

/// Install the embedded plugin to the default path if needed.
///
/// Skips if `PINENTRY_ZELLIJ_PLUGIN` is set (user manages their own path).
/// Errors are non-fatal — the plugin may already be installed from a prior
/// run, or the user may have placed it manually.
pub fn ensure_plugin_installed() -> Result<(), EmbedError> {
    // If the user set a custom plugin path, don't touch it.
    if std::env::var_os("PINENTRY_ZELLIJ_PLUGIN").is_some() {
        return Ok(());
    }

    let target = default_plugin_path()?;
    ensure_plugin(&target)?;

    // Always ensure permissions exist (even if plugin was already up to date).
    #[cfg(not(test))]
    ensure_permissions(&target);

    Ok(())
}

/// Resolve the default plugin install path.
fn default_plugin_path() -> Result<PathBuf, EmbedError> {
    let config = dirs::config_dir().ok_or(EmbedError::NoConfigDir)?;
    Ok(config
        .join("zellij")
        .join("plugins")
        .join("pinentry-zellij-plugin.wasm"))
}

/// Write the embedded plugin to `target` if the installed version is missing
/// or doesn't match the embedded hash.
pub fn ensure_plugin(target: &Path) -> Result<(), EmbedError> {
    // Fast path: file exists and xattr hash matches.
    if target.exists()
        && let Ok(Some(stored)) = xattr::get(target, XATTR_NAME)
        && stored == PLUGIN_HASH.as_bytes()
    {
        debug!("plugin up to date at {}", target.display());
        return Ok(());
    }

    let parent = target.parent().ok_or(EmbedError::NoParentDir)?;
    fs::create_dir_all(parent)?;

    // Atomic write: tempfile in the same dir, persist into place.
    let tmp = NamedTempFile::new_in(parent)?;
    fs::write(tmp.path(), PLUGIN_BYTES)?;
    tmp.persist(target)?;

    // Set version xattr. Ignore errors (fs may not support xattrs).
    if let Err(e) = xattr::set(target, XATTR_NAME, PLUGIN_HASH.as_bytes()) {
        warn!("could not set xattr on {}: {e}", target.display());
    }

    debug!("installed plugin to {}", target.display());
    Ok(())
}

/// Build a KDL permissions entry for the given plugin path.
///
/// Returns `(quoted_key, entry)` where `quoted_key` is the string to check
/// for duplicates and `entry` is the full KDL block to append.
#[cfg_attr(test, allow(dead_code))]
fn build_permissions_entry(plugin_path: &Path) -> (String, String) {
    let path_str = plugin_path.to_string_lossy();
    let quoted_key = format!("\"{path_str}\"");
    let entry = format!(
        "\n\"{path_str}\" {{\n    ReadCliPipes\n    ReadApplicationState\n    ChangeApplicationState\n}}\n"
    );
    (quoted_key, entry)
}

/// Write the zellij permissions cache so our plugin is pre-granted.
///
/// Opens the file once for reading and appending to narrow the race window
/// between checking and writing. Duplicate entries are harmless if a race
/// still occurs between concurrent instances.
#[cfg(not(test))]
fn ensure_permissions(plugin_path: &Path) {
    let Some(cache_dir) = dirs::cache_dir() else {
        return;
    };
    let perms_path = cache_dir.join("zellij").join("permissions.kdl");

    let (quoted_key, entry) = build_permissions_entry(plugin_path);

    if let Some(parent) = perms_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let mut file = match fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&perms_path)
    {
        Ok(f) => f,
        Err(e) => {
            warn!("could not open permissions cache: {e}");
            return;
        }
    };

    use std::io::{Read, Write};
    let mut contents = String::new();
    if file.read_to_string(&mut contents).is_ok() && contents.contains(&quoted_key) {
        return;
    }

    if let Err(e) = file.write_all(entry.as_bytes()) {
        warn!("could not write permissions cache: {e}");
    } else {
        debug!("wrote zellij permissions for {}", plugin_path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    use std::sync::atomic::{AtomicU32, Ordering};
    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn tmpdir() -> std::path::PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("pinentry-zellij-test-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn fresh_install() {
        let dir = tmpdir();
        let target = dir.join("plugin.wasm");

        ensure_plugin(&target).unwrap();

        assert!(target.exists());
        assert_eq!(fs::read(&target).unwrap(), PLUGIN_BYTES);

        // xattr should be set (may fail on some filesystems)
        if let Ok(Some(stored)) = xattr::get(&target, XATTR_NAME) {
            assert_eq!(stored, PLUGIN_HASH.as_bytes());
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hash_match_skips_rewrite() {
        let dir = tmpdir();
        let target = dir.join("plugin.wasm");

        // First install.
        ensure_plugin(&target).unwrap();
        let mtime1 = fs::metadata(&target)
            .unwrap()
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);

        // Brief pause so mtime would differ on rewrite.
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Second call should skip.
        ensure_plugin(&target).unwrap();
        let mtime2 = fs::metadata(&target)
            .unwrap()
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);

        // xattr-based skip: mtime unchanged (if xattrs are supported).
        if xattr::get(&target, XATTR_NAME).is_ok() {
            assert_eq!(mtime1, mtime2, "file should not have been rewritten");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hash_mismatch_rewrites() {
        let dir = tmpdir();
        let target = dir.join("plugin.wasm");

        // Write a fake plugin with wrong hash.
        fs::write(&target, b"old content").unwrap();
        let _ = xattr::set(&target, XATTR_NAME, b"wrong_hash");

        ensure_plugin(&target).unwrap();

        assert_eq!(fs::read(&target).unwrap(), PLUGIN_BYTES);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_xattr_rewrites() {
        let dir = tmpdir();
        let target = dir.join("plugin.wasm");

        // Write a file with no xattr.
        fs::write(&target, b"no xattr").unwrap();

        ensure_plugin(&target).unwrap();

        assert_eq!(fs::read(&target).unwrap(), PLUGIN_BYTES);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn creates_parent_dirs() {
        let dir = tmpdir();
        let target = dir.join("a").join("b").join("plugin.wasm");

        ensure_plugin(&target).unwrap();

        assert!(target.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_env_skips_install() {
        let dir = tmpdir();
        let target = dir.join("plugin.wasm");

        temp_env::with_var("PINENTRY_ZELLIJ_PLUGIN", Some("/custom/path.wasm"), || {
            ensure_plugin_installed().unwrap();
        });

        assert!(!target.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn permissions_entry_structure() {
        let path = Path::new("/home/user/.config/zellij/plugins/pinentry-zellij-plugin.wasm");
        let (key, entry) = build_permissions_entry(path);

        assert_eq!(
            key,
            "\"/home/user/.config/zellij/plugins/pinentry-zellij-plugin.wasm\""
        );
        assert!(entry.contains(&key));
        assert!(entry.contains("ReadCliPipes"));
        assert!(entry.contains("ReadApplicationState"));
        assert!(entry.contains("ChangeApplicationState"));
    }

    #[test]
    fn permissions_entry_is_valid_kdl_block() {
        let path = Path::new("/tmp/plugin.wasm");
        let (_, entry) = build_permissions_entry(path);

        // Should have opening and closing braces on separate lines.
        assert!(entry.contains("{\n"));
        assert!(entry.contains("\n}"));
        // Each permission indented.
        for perm in [
            "ReadCliPipes",
            "ReadApplicationState",
            "ChangeApplicationState",
        ] {
            assert!(entry.contains(&format!("    {perm}")));
        }
    }

    #[test]
    fn permissions_key_detects_duplicates() {
        let path = Path::new("/tmp/plugin.wasm");
        let (key, entry) = build_permissions_entry(path);

        // Simulated existing file content containing this plugin.
        let existing = format!("some_other_plugin {{}}\n{entry}");
        assert!(existing.contains(&key));

        // Different plugin should not match.
        let other = "some_other_plugin {}".to_string();
        assert!(!other.contains(&key));
    }
}
