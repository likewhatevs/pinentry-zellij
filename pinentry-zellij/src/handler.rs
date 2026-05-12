//! Handler dispatch: routes pinentry requests to Zellij plugin or TTY fallback.

use std::io::Write;
use std::process::{Command, Stdio};

use zeroize::Zeroize;

use crate::assuan::HandlerResult;
use crate::protocol::{PinStatus, PinentryCmd, PinentryRequest};
use crate::state::PinentryState;
use crate::tty::{self, RealTtyIo};
use crate::zellij;

/// Dispatch a pinentry request: try Zellij first, fall back to TTY.
pub fn dispatch(state: &PinentryState) -> HandlerResult {
    if zellij::in_zellij() {
        match try_zellij(state) {
            Some(result) => return result,
            None => {
                tracing::warn!("plugin failed, falling back to TTY");
            }
        }
    }
    tty::handle_tty(state, &mut RealTtyIo)
}

/// Attempt to handle the request via the Zellij plugin.
///
/// Returns `None` if any step fails (plugin not found, command failed,
/// invalid JSON response), allowing the caller to fall back to TTY.
fn try_zellij(state: &PinentryState) -> Option<HandlerResult> {
    let request = state.to_request();
    let plugin = zellij::plugin_path();

    // Single command: zellij pipe --plugin launches the plugin as a floating
    // pane if not running, sends the request, and blocks until the plugin
    // responds via cli_pipe_output + unblock_cli_pipe_input.
    let pipe_args = zellij::build_pipe_args(&plugin);
    let mut payload = serde_json::to_string(&request).expect("serialize request");

    tracing::debug!("running zellij pipe");

    // Pipe the JSON payload via stdin rather than as a positional arg.
    // Command::output() sets stdin to null, and zellij pipe prioritizes
    // stdin over argv when it detects piped input (!is_terminal). Sending
    // the payload through stdin works in both interactive mode (terminal
    // parent stdin) and assuan mode (gpg-agent pipe parent stdin) without
    // stealing bytes from the parent's stdin.
    let child = Command::new("zellij")
        .args(&pipe_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let output = pipe_payload_and_collect(child, &mut payload)?;

    tracing::debug!(
        "zellij pipe exited: status={}, stdout_len={}",
        output.status,
        output.stdout.len()
    );

    let mut stdout = match String::from_utf8(output.stdout) {
        Ok(s) => s,
        Err(e) => {
            e.into_bytes().zeroize();
            return None;
        }
    };
    let response = zellij::parse_pipe_response(&stdout).ok()?;
    stdout.zeroize();

    Some(response_to_handler_result(&request, response))
}

/// Write `payload` to the child's stdin, close the pipe, and collect output.
///
/// Zeroizes `payload` regardless of outcome. On write failure, reaps the
/// child before returning `None`.
fn pipe_payload_and_collect(
    mut child: std::process::Child,
    payload: &mut String,
) -> Option<std::process::Output> {
    let write_ok = match child.stdin.take() {
        Some(mut stdin) => stdin.write_all(payload.as_bytes()).is_ok(),
        None => false,
    };
    payload.zeroize();

    if !write_ok {
        let _ = child.wait();
        return None;
    }

    child.wait_with_output().ok()
}

/// Convert a plugin response into a handler result.
fn response_to_handler_result(
    request: &PinentryRequest,
    response: crate::protocol::PinResponse,
) -> HandlerResult {
    match response.status {
        PinStatus::Ok => {
            if let Some(passphrase) = response.passphrase {
                HandlerResult::Pin(zeroize::Zeroizing::new(passphrase))
            } else {
                match request.cmd {
                    PinentryCmd::GetPin => HandlerResult::Canceled,
                    PinentryCmd::Confirm | PinentryCmd::Message => HandlerResult::Confirmed,
                }
            }
        }
        PinStatus::Canceled => HandlerResult::Canceled,
        PinStatus::NotConfirmed => HandlerResult::NotConfirmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{PinResponse, PinStatus, PinentryCmd};

    #[test]
    fn response_to_handler_pin_ok() {
        let req = PinentryRequest {
            cmd: PinentryCmd::GetPin,
            title: None,
            desc: None,
            prompt: None,
            error: None,
            ok: None,
            cancel: None,
            notok: None,
            repeat: None,
            repeat_error: None,
        };
        let resp = PinResponse {
            status: PinStatus::Ok,
            passphrase: Some("pw".into()),
        };
        let result = response_to_handler_result(&req, resp);
        match result {
            HandlerResult::Pin(p) => assert_eq!(&*p, "pw"),
            _ => panic!("expected Pin"),
        }
    }

    #[test]
    fn response_to_handler_confirm_ok() {
        let req = PinentryRequest {
            cmd: PinentryCmd::Confirm,
            title: None,
            desc: None,
            prompt: None,
            error: None,
            ok: None,
            cancel: None,
            notok: None,
            repeat: None,
            repeat_error: None,
        };
        let resp = PinResponse {
            status: PinStatus::Ok,
            passphrase: None,
        };
        let result = response_to_handler_result(&req, resp);
        assert!(matches!(result, HandlerResult::Confirmed));
    }

    #[test]
    fn response_to_handler_canceled() {
        let req = PinentryRequest {
            cmd: PinentryCmd::GetPin,
            title: None,
            desc: None,
            prompt: None,
            error: None,
            ok: None,
            cancel: None,
            notok: None,
            repeat: None,
            repeat_error: None,
        };
        let resp = PinResponse {
            status: PinStatus::Canceled,
            passphrase: None,
        };
        let result = response_to_handler_result(&req, resp);
        assert!(matches!(result, HandlerResult::Canceled));
    }

    #[test]
    fn response_to_handler_not_confirmed() {
        let req = PinentryRequest {
            cmd: PinentryCmd::Confirm,
            title: None,
            desc: None,
            prompt: None,
            error: None,
            ok: None,
            cancel: None,
            notok: None,
            repeat: None,
            repeat_error: None,
        };
        let resp = PinResponse {
            status: PinStatus::NotConfirmed,
            passphrase: None,
        };
        let result = response_to_handler_result(&req, resp);
        assert!(matches!(result, HandlerResult::NotConfirmed));
    }

    #[test]
    fn response_getpin_ok_no_passphrase_is_canceled() {
        let req = PinentryRequest {
            cmd: PinentryCmd::GetPin,
            title: None,
            desc: None,
            prompt: None,
            error: None,
            ok: None,
            cancel: None,
            notok: None,
            repeat: None,
            repeat_error: None,
        };
        let resp = PinResponse {
            status: PinStatus::Ok,
            passphrase: None,
        };
        let result = response_to_handler_result(&req, resp);
        assert!(matches!(result, HandlerResult::Canceled));
    }

    #[test]
    fn response_message_ok_is_confirmed() {
        let req = PinentryRequest {
            cmd: PinentryCmd::Message,
            title: None,
            desc: None,
            prompt: None,
            error: None,
            ok: None,
            cancel: None,
            notok: None,
            repeat: None,
            repeat_error: None,
        };
        let resp = PinResponse {
            status: PinStatus::Ok,
            passphrase: None,
        };
        let result = response_to_handler_result(&req, resp);
        assert!(matches!(result, HandlerResult::Confirmed));
    }

    #[test]
    fn pipe_payload_echoed_back() {
        let child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut payload = "hello world".to_string();
        let output = pipe_payload_and_collect(child, &mut payload).unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "hello world");
        assert!(output.status.success());
    }

    #[test]
    fn pipe_payload_zeroizes() {
        let child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut payload = "sensitive data".to_string();
        let _ = pipe_payload_and_collect(child, &mut payload);
        assert!(payload.bytes().all(|b| b == 0));
    }

    #[test]
    fn pipe_payload_json_roundtrip() {
        let req = PinentryRequest {
            cmd: PinentryCmd::GetPin,
            title: Some("Title".into()),
            desc: Some("Enter passphrase".into()),
            prompt: Some("PIN:".into()),
            error: None,
            ok: None,
            cancel: None,
            notok: None,
            repeat: None,
            repeat_error: None,
        };
        let child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut payload = serde_json::to_string(&req).unwrap();
        let output = pipe_payload_and_collect(child, &mut payload).unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let parsed: PinentryRequest = serde_json::from_str(&stdout).unwrap();
        assert_eq!(parsed.cmd, PinentryCmd::GetPin);
        assert_eq!(parsed.title.as_deref(), Some("Title"));
        assert_eq!(parsed.desc.as_deref(), Some("Enter passphrase"));
        assert_eq!(parsed.prompt.as_deref(), Some("PIN:"));
    }

    #[test]
    fn pipe_payload_write_to_exited_process() {
        // Spawn a process that exits immediately, then write to the closed pipe.
        let mut child = Command::new("true")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Save stdin handle before wait() drops it.
        let stdin_handle = child.stdin.take();
        let _ = child.wait();
        // Restore stdin — the read end is closed since the child exited,
        // so write_all will get EPIPE.
        child.stdin = stdin_handle;
        let mut payload = "data".to_string();
        let result = pipe_payload_and_collect(child, &mut payload);
        assert!(result.is_none());
        // Payload is zeroized even on write failure.
        assert!(payload.bytes().all(|b| b == 0));
    }
}
