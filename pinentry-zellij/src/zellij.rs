//! Zellij pipe integration.
//!
//! Builds the CLI arguments for `zellij pipe` and parses the JSON response
//! from the plugin. The plugin path is determined from the
//! `PINENTRY_ZELLIJ_PLUGIN` environment variable or a default location.

use crate::protocol::{PinResponse, PinentryRequest};

const DEFAULT_PLUGIN_DIR: &str = ".config/zellij/plugins";
const PLUGIN_FILENAME: &str = "pinentry-zellij-plugin.wasm";

/// Determine the plugin URL for `zellij pipe --plugin`.
///
/// Returns a `file:` URL as required by zellij pipe. Non-file URL schemes
/// in `PINENTRY_ZELLIJ_PLUGIN` are rejected (falls back to default).
pub fn plugin_path() -> String {
    if let Ok(path) = std::env::var("PINENTRY_ZELLIJ_PLUGIN") {
        if path.starts_with("file:") {
            return path;
        }
        if path.contains("://") {
            tracing::warn!("PINENTRY_ZELLIJ_PLUGIN: only file: scheme supported, using default");
        } else {
            return format!("file:{path}");
        }
    }
    if let Some(home) = dirs::home_dir() {
        let path = home.join(DEFAULT_PLUGIN_DIR).join(PLUGIN_FILENAME);
        return format!("file:{}", path.display());
    }
    format!("file:~/{DEFAULT_PLUGIN_DIR}/{PLUGIN_FILENAME}")
}

/// Build the argument list for `zellij pipe`.
///
/// When `plugin` is provided, uses `--plugin` to target that specific
/// plugin (avoids broadcast which causes other plugins to unblock the pipe).
pub fn build_pipe_args(request: &PinentryRequest, plugin: &str) -> Vec<String> {
    let payload = serde_json::to_string(request).expect("serialize request");
    vec![
        "pipe".into(),
        "--plugin".into(),
        plugin.into(),
        "--name".into(),
        "pinentry".into(),
        "--".into(),
        payload,
    ]
}

/// Parse the stdout of `zellij pipe` into a PinResponse.
pub fn parse_pipe_response(output: &str) -> Result<PinResponse, String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Err("empty response from plugin".into());
    }
    serde_json::from_str(trimmed).map_err(|e| format!("failed to parse plugin response: {e}"))
}

/// Check if we're running inside a Zellij session.
pub fn in_zellij() -> bool {
    std::env::var("ZELLIJ_SESSION_NAME").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{PinStatus, PinentryCmd};

    #[test]
    fn build_pipe_args_structure() {
        let req = PinentryRequest {
            cmd: PinentryCmd::GetPin,
            title: None,
            desc: Some("Enter passphrase".into()),
            prompt: Some("PIN:".into()),
            error: None,
            ok: None,
            cancel: None,
            notok: None,
            repeat: None,
            repeat_error: None,
        };
        let args = build_pipe_args(&req, "file:/path/to/plugin.wasm");
        assert_eq!(args[0], "pipe");
        assert_eq!(args[1], "--plugin");
        assert_eq!(args[2], "file:/path/to/plugin.wasm");
        assert_eq!(args[3], "--name");
        assert_eq!(args[4], "pinentry");
        assert_eq!(args[5], "--");
        let _: PinentryRequest = serde_json::from_str(&args[6]).unwrap();
    }

    #[test]
    fn parse_pipe_response_ok() {
        let json = r#"{"status":"Ok","passphrase":"secret"}"#;
        let resp = parse_pipe_response(json).unwrap();
        assert_eq!(resp.status, PinStatus::Ok);
        assert_eq!(resp.passphrase.as_deref(), Some("secret"));
    }

    #[test]
    fn parse_pipe_response_canceled() {
        let json = r#"{"status":"Canceled"}"#;
        let resp = parse_pipe_response(json).unwrap();
        assert_eq!(resp.status, PinStatus::Canceled);
        assert!(resp.passphrase.is_none());
    }

    #[test]
    fn parse_pipe_response_with_whitespace() {
        let json = "  \n{\"status\":\"Ok\",\"passphrase\":\"pw\"}\n  ";
        let resp = parse_pipe_response(json).unwrap();
        assert_eq!(resp.status, PinStatus::Ok);
    }

    #[test]
    fn parse_pipe_response_empty() {
        assert!(parse_pipe_response("").is_err());
        assert!(parse_pipe_response("  \n  ").is_err());
    }

    #[test]
    fn parse_pipe_response_invalid_json() {
        assert!(parse_pipe_response("not json").is_err());
    }

    #[test]
    fn plugin_path_from_env() {
        temp_env::with_var("PINENTRY_ZELLIJ_PLUGIN", Some("/custom/path.wasm"), || {
            assert_eq!(plugin_path(), "file:/custom/path.wasm");
        });
    }

    #[test]
    fn plugin_path_from_env_with_scheme() {
        temp_env::with_var(
            "PINENTRY_ZELLIJ_PLUGIN",
            Some("file:/custom/path.wasm"),
            || {
                assert_eq!(plugin_path(), "file:/custom/path.wasm");
            },
        );
    }

    #[test]
    fn plugin_path_default() {
        temp_env::with_vars(
            [
                ("PINENTRY_ZELLIJ_PLUGIN", None::<&str>),
                ("HOME", Some("/home/testuser")),
            ],
            || {
                assert_eq!(
                    plugin_path(),
                    "file:/home/testuser/.config/zellij/plugins/pinentry-zellij-plugin.wasm"
                );
            },
        );
    }

    #[test]
    fn in_zellij_true() {
        temp_env::with_var("ZELLIJ_SESSION_NAME", Some("test-session"), || {
            assert!(in_zellij());
        });
    }

    #[test]
    fn in_zellij_false() {
        temp_env::with_var("ZELLIJ_SESSION_NAME", None::<&str>, || {
            assert!(!in_zellij());
        });
    }

    #[test]
    fn plugin_path_default_uses_home() {
        temp_env::with_vars([("PINENTRY_ZELLIJ_PLUGIN", None::<&str>)], || {
            let path = plugin_path();
            assert!(path.contains("pinentry-zellij-plugin.wasm"));
            assert!(path.starts_with("file:/"));
        });
    }

    #[test]
    fn plugin_path_rejects_non_file_scheme() {
        temp_env::with_vars(
            [
                ("PINENTRY_ZELLIJ_PLUGIN", Some("http://evil.com/bad.wasm")),
                ("HOME", Some("/home/testuser")),
            ],
            || {
                let path = plugin_path();
                assert!(!path.contains("http"));
                assert!(path.starts_with("file:"));
            },
        );
    }
}
