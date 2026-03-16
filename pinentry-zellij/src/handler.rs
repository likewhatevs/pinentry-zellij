//! Handler dispatch: routes pinentry requests to Zellij plugin or TTY fallback.

use std::process::Command;

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
    let pipe_args = zellij::build_pipe_args(&request, &plugin);
    tracing::debug!("running zellij pipe");
    // stdin is inherited (not null) because zellij pipe's client checks
    // is_terminal() on stdin. With /dev/null it thinks stdin is piped,
    // reads EOF, and exits before the plugin can respond.
    // In Assuan mode, stdin is gpg-agent's pipe (not a terminal), so
    // the pipe client sees is_piped=true, reads EOF, sends one empty
    // message, then enters the response loop. The payload was in argv
    // so no protocol bytes are stolen.
    let output = Command::new("zellij").args(&pipe_args).output().ok()?;

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
}
