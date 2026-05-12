//! SSH_ASKPASS / SUDO_ASKPASS mode.
//!
//! When invoked with a command-line argument (the prompt), acts as an askpass
//! program: displays the prompt, reads a passphrase, prints it to stdout,
//! and exits with 0 on success or 1 on cancel.

use std::process::ExitCode;

use zeroize::Zeroizing;

use crate::assuan::HandlerResult;
use crate::handler;
use crate::state::PinentryState;

/// Run in askpass mode: prompt for a passphrase and print it to stdout.
pub fn run(prompt: &str) -> ExitCode {
    let mut state = PinentryState::default();
    // The askpass arg is the description/context for the user (e.g.
    // "Enter passphrase for '/path/to/key':"). Render it as the dialog
    // body and let the field fall back to the generic "Passphrase:" label —
    // matching pre-existing layout, just without the literal "PIN:".
    if !prompt.is_empty() {
        state.desc = Some(prompt.to_string());
    }
    state.set_cmd_getpin();

    let (output, code) = result_to_output(handler::dispatch(&state));

    if let Some(passphrase) = output {
        // Print by reference — the Zeroizing wrapper zeroizes on drop.
        println!("{}", &*passphrase);
    }

    code
}

/// Convert a handler result to (optional passphrase, exit code).
///
/// The passphrase stays in a `Zeroizing` wrapper so the caller can print
/// it and have the memory zeroized on drop.
fn result_to_output(result: HandlerResult) -> (Option<Zeroizing<String>>, ExitCode) {
    match result {
        HandlerResult::Pin(passphrase) => (Some(passphrase), ExitCode::SUCCESS),
        HandlerResult::Confirmed | HandlerResult::Canceled | HandlerResult::NotConfirmed => {
            (None, ExitCode::FAILURE)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_result_produces_output_and_success() {
        let result = HandlerResult::Pin(Zeroizing::new("secret".into()));
        let (output, code) = result_to_output(result);
        assert_eq!(&*output.unwrap(), "secret");
        assert_eq!(code, ExitCode::SUCCESS);
    }

    #[test]
    fn canceled_produces_no_output_and_failure() {
        let (output, code) = result_to_output(HandlerResult::Canceled);
        assert!(output.is_none());
        assert_eq!(code, ExitCode::FAILURE);
    }

    #[test]
    fn not_confirmed_produces_failure() {
        let (output, code) = result_to_output(HandlerResult::NotConfirmed);
        assert!(output.is_none());
        assert_eq!(code, ExitCode::FAILURE);
    }

    #[test]
    fn confirmed_produces_failure() {
        let (output, code) = result_to_output(HandlerResult::Confirmed);
        assert!(output.is_none());
        assert_eq!(code, ExitCode::FAILURE);
    }
}
