//! TTY-based pinentry fallback.
//!
//! Used when not running inside Zellij, or when the Zellij plugin fails.
//! Prompts the user on the TTY via stderr/stdin using rpassword for
//! hidden passphrase input.

use crate::assuan::HandlerResult;
use crate::protocol::PinentryCmd;
use crate::state::PinentryState;

/// Abstraction over TTY I/O for testing.
pub trait TtyIo {
    /// Write a message to stderr (visible to the user).
    fn write_stderr(&mut self, msg: &str);
    /// Read a passphrase with echo disabled.
    fn read_password(&mut self, prompt: &str) -> Option<String>;
    /// Read a line of text (with echo).
    fn read_line(&mut self, prompt: &str) -> Option<String>;
}

/// Real TTY I/O using stderr and rpassword.
pub struct RealTtyIo;

impl TtyIo for RealTtyIo {
    fn write_stderr(&mut self, msg: &str) {
        eprint!("{msg}");
    }

    fn read_password(&mut self, prompt: &str) -> Option<String> {
        rpassword::prompt_password(prompt).ok()
    }

    fn read_line(&mut self, prompt: &str) -> Option<String> {
        self.write_stderr(prompt);
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf).ok()?;
        Some(buf.trim_end().to_string())
    }
}

/// Handle a pinentry request via the TTY.
pub fn handle_tty(state: &PinentryState, io: &mut dyn TtyIo) -> HandlerResult {
    match state.cmd() {
        Some(PinentryCmd::GetPin) => handle_getpin(state, io),
        Some(PinentryCmd::Confirm) => handle_confirm(state, io),
        Some(PinentryCmd::Message) => handle_message(state, io),
        None => HandlerResult::Canceled,
    }
}

fn handle_getpin(state: &PinentryState, io: &mut dyn TtyIo) -> HandlerResult {
    if let Some(desc) = &state.desc {
        io.write_stderr(&format!("{desc}\n"));
    }
    if let Some(error) = &state.error {
        io.write_stderr(&format!("*** {error} ***\n"));
    }

    let prompt = state.prompt.as_deref().unwrap_or("PIN:");
    let prompt = format_prompt(prompt);

    let Some(passphrase) = io.read_password(&prompt).map(zeroize::Zeroizing::new) else {
        return HandlerResult::Canceled;
    };

    if let Some(repeat_prompt) = &state.repeat {
        let repeat_prompt = format_prompt(repeat_prompt);
        let passphrase2 = match io.read_password(&repeat_prompt) {
            Some(p) => zeroize::Zeroizing::new(p),
            None => return HandlerResult::Canceled,
        };
        if *passphrase != *passphrase2 {
            let err = state
                .repeat_error
                .as_deref()
                .unwrap_or("Passphrases don't match.");
            io.write_stderr(&format!("*** {err} ***\n"));
            return HandlerResult::Canceled;
        }
    }

    HandlerResult::Pin(passphrase)
}

fn handle_confirm(state: &PinentryState, io: &mut dyn TtyIo) -> HandlerResult {
    if let Some(desc) = &state.desc {
        io.write_stderr(&format!("{desc}\n"));
    }

    let ok_label = state.ok.as_deref().unwrap_or("OK");
    let cancel_label = state.cancel.as_deref().unwrap_or("Cancel");

    let prompt = if state.notok.is_some() {
        let notok_label = state.notok.as_deref().unwrap();
        format!("[y]{ok_label} [n]{notok_label} [c]{cancel_label}? ")
    } else {
        format!("[y]{ok_label} [n]{cancel_label}? ")
    };

    let Some(answer) = io.read_line(&prompt) else {
        return HandlerResult::Canceled;
    };

    match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => HandlerResult::Confirmed,
        "n" | "no" => {
            if state.notok.is_some() {
                HandlerResult::NotConfirmed
            } else {
                HandlerResult::Canceled
            }
        }
        _ => HandlerResult::Canceled,
    }
}

fn handle_message(state: &PinentryState, io: &mut dyn TtyIo) -> HandlerResult {
    if let Some(desc) = &state.desc {
        io.write_stderr(&format!("{desc}\n"));
    }
    io.write_stderr("Press enter to continue.");
    let _ = io.read_line("");
    HandlerResult::Confirmed
}

/// Ensure prompt ends with `:` or `?` followed by a space.
fn format_prompt(prompt: &str) -> String {
    if prompt.ends_with(':') || prompt.ends_with('?') {
        format!("{prompt} ")
    } else {
        format!("{prompt}: ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockTtyIo {
        stderr: Vec<String>,
        passwords: Vec<String>,
        lines: Vec<String>,
        pw_idx: usize,
        line_idx: usize,
    }

    impl MockTtyIo {
        fn new(passwords: Vec<&str>, lines: Vec<&str>) -> Self {
            Self {
                stderr: Vec::new(),
                passwords: passwords.into_iter().map(String::from).collect(),
                lines: lines.into_iter().map(String::from).collect(),
                pw_idx: 0,
                line_idx: 0,
            }
        }

        fn stderr_output(&self) -> String {
            self.stderr.join("")
        }
    }

    impl TtyIo for MockTtyIo {
        fn write_stderr(&mut self, msg: &str) {
            self.stderr.push(msg.to_string());
        }

        fn read_password(&mut self, _prompt: &str) -> Option<String> {
            if self.pw_idx < self.passwords.len() {
                let p = self.passwords[self.pw_idx].clone();
                self.pw_idx += 1;
                Some(p)
            } else {
                None
            }
        }

        fn read_line(&mut self, _prompt: &str) -> Option<String> {
            if self.line_idx < self.lines.len() {
                let l = self.lines[self.line_idx].clone();
                self.line_idx += 1;
                Some(l)
            } else {
                None
            }
        }
    }

    #[test]
    fn getpin_basic() {
        let mut io = MockTtyIo::new(vec!["secret"], vec![]);
        let mut state = PinentryState::default();
        state.desc = Some("Enter passphrase".into());
        state.set_cmd_getpin();

        let result = handle_tty(&state, &mut io);
        match result {
            HandlerResult::Pin(p) => assert_eq!(&*p, "secret"),
            _ => panic!("expected Pin"),
        }
        assert!(io.stderr_output().contains("Enter passphrase"));
    }

    #[test]
    fn getpin_with_error() {
        let mut io = MockTtyIo::new(vec!["secret"], vec![]);
        let mut state = PinentryState::default();
        state.error = Some("Bad passphrase".into());
        state.set_cmd_getpin();

        let result = handle_tty(&state, &mut io);
        match result {
            HandlerResult::Pin(p) => assert_eq!(&*p, "secret"),
            _ => panic!("expected Pin"),
        }
        assert!(io.stderr_output().contains("Bad passphrase"));
    }

    #[test]
    fn getpin_repeat_match() {
        let mut io = MockTtyIo::new(vec!["secret", "secret"], vec![]);
        let mut state = PinentryState::default();
        state.repeat = Some("Repeat:".into());
        state.set_cmd_getpin();

        let result = handle_tty(&state, &mut io);
        match result {
            HandlerResult::Pin(p) => assert_eq!(&*p, "secret"),
            _ => panic!("expected Pin"),
        }
    }

    #[test]
    fn getpin_repeat_mismatch() {
        let mut io = MockTtyIo::new(vec!["secret", "wrong"], vec![]);
        let mut state = PinentryState::default();
        state.repeat = Some("Repeat:".into());
        state.set_cmd_getpin();

        let result = handle_tty(&state, &mut io);
        assert!(matches!(result, HandlerResult::Canceled));
        assert!(io.stderr_output().contains("don't match"));
    }

    #[test]
    fn getpin_repeat_mismatch_custom_error() {
        let mut io = MockTtyIo::new(vec!["a", "b"], vec![]);
        let mut state = PinentryState::default();
        state.repeat = Some("Again:".into());
        state.repeat_error = Some("No match!".into());
        state.set_cmd_getpin();

        let result = handle_tty(&state, &mut io);
        assert!(matches!(result, HandlerResult::Canceled));
        assert!(io.stderr_output().contains("No match!"));
    }

    #[test]
    fn getpin_eof() {
        let mut io = MockTtyIo::new(vec![], vec![]);
        let mut state = PinentryState::default();
        state.set_cmd_getpin();

        let result = handle_tty(&state, &mut io);
        assert!(matches!(result, HandlerResult::Canceled));
    }

    #[test]
    fn confirm_yes() {
        let mut io = MockTtyIo::new(vec![], vec!["y"]);
        let mut state = PinentryState::default();
        state.desc = Some("Trust?".into());
        state.set_cmd_confirm();

        let result = handle_tty(&state, &mut io);
        assert!(matches!(result, HandlerResult::Confirmed));
    }

    #[test]
    fn confirm_no_without_notok() {
        let mut io = MockTtyIo::new(vec![], vec!["n"]);
        let mut state = PinentryState::default();
        state.set_cmd_confirm();

        let result = handle_tty(&state, &mut io);
        assert!(matches!(result, HandlerResult::Canceled));
    }

    #[test]
    fn confirm_no_with_notok() {
        let mut io = MockTtyIo::new(vec![], vec!["n"]);
        let mut state = PinentryState::default();
        state.notok = Some("No way".into());
        state.set_cmd_confirm();

        let result = handle_tty(&state, &mut io);
        assert!(matches!(result, HandlerResult::NotConfirmed));
    }

    #[test]
    fn confirm_cancel() {
        let mut io = MockTtyIo::new(vec![], vec!["c"]);
        let mut state = PinentryState::default();
        state.notok = Some("Nope".into());
        state.set_cmd_confirm();

        let result = handle_tty(&state, &mut io);
        assert!(matches!(result, HandlerResult::Canceled));
    }

    #[test]
    fn confirm_eof() {
        let mut io = MockTtyIo::new(vec![], vec![]);
        let mut state = PinentryState::default();
        state.set_cmd_confirm();

        let result = handle_tty(&state, &mut io);
        assert!(matches!(result, HandlerResult::Canceled));
    }

    #[test]
    fn message_basic() {
        let mut io = MockTtyIo::new(vec![], vec![""]);
        let mut state = PinentryState::default();
        state.desc = Some("Information".into());
        state.set_cmd_message();

        let result = handle_tty(&state, &mut io);
        assert!(matches!(result, HandlerResult::Confirmed));
        assert!(io.stderr_output().contains("Information"));
    }

    #[test]
    fn no_cmd_returns_canceled() {
        let mut io = MockTtyIo::new(vec![], vec![]);
        let state = PinentryState::default();

        let result = handle_tty(&state, &mut io);
        assert!(matches!(result, HandlerResult::Canceled));
    }

    #[test]
    fn format_prompt_with_colon() {
        assert_eq!(format_prompt("PIN:"), "PIN: ");
    }

    #[test]
    fn format_prompt_without_colon() {
        assert_eq!(format_prompt("Passphrase"), "Passphrase: ");
    }

    #[test]
    fn format_prompt_with_question() {
        assert_eq!(format_prompt("Password?"), "Password? ");
    }

    #[test]
    fn getpin_repeat_eof_on_second() {
        let mut io = MockTtyIo::new(vec!["secret"], vec![]);
        let mut state = PinentryState::default();
        state.repeat = Some("Again:".into());
        state.set_cmd_getpin();

        let result = handle_tty(&state, &mut io);
        assert!(matches!(result, HandlerResult::Canceled));
    }

    #[test]
    fn getpin_no_desc_no_error() {
        let mut io = MockTtyIo::new(vec!["pw"], vec![]);
        let mut state = PinentryState::default();
        state.set_cmd_getpin();

        let result = handle_tty(&state, &mut io);
        match result {
            HandlerResult::Pin(p) => assert_eq!(&*p, "pw"),
            _ => panic!("expected Pin"),
        }
        // Should not have written any desc/error
        assert!(io.stderr_output().is_empty());
    }

    #[test]
    fn confirm_with_custom_labels() {
        let mut io = MockTtyIo::new(vec![], vec!["y"]);
        let mut state = PinentryState::default();
        state.ok = Some("Accept".into());
        state.cancel = Some("Reject".into());
        state.set_cmd_confirm();

        let result = handle_tty(&state, &mut io);
        assert!(matches!(result, HandlerResult::Confirmed));
    }

    #[test]
    fn confirm_unknown_input() {
        let mut io = MockTtyIo::new(vec![], vec!["maybe"]);
        let mut state = PinentryState::default();
        state.set_cmd_confirm();

        let result = handle_tty(&state, &mut io);
        assert!(matches!(result, HandlerResult::Canceled));
    }

    #[test]
    fn message_no_desc() {
        let mut io = MockTtyIo::new(vec![], vec![""]);
        let mut state = PinentryState::default();
        state.set_cmd_message();

        let result = handle_tty(&state, &mut io);
        assert!(matches!(result, HandlerResult::Confirmed));
        // Should show "Press enter" but no desc
        let output = io.stderr_output();
        assert!(output.contains("Press enter"));
        assert!(!output.contains('\n'));
    }

    #[test]
    fn message_eof() {
        let mut io = MockTtyIo::new(vec![], vec![]);
        let mut state = PinentryState::default();
        state.set_cmd_message();

        let result = handle_tty(&state, &mut io);
        // Message always returns Confirmed even on EOF
        assert!(matches!(result, HandlerResult::Confirmed));
    }
}
