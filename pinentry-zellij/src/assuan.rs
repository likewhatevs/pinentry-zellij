//! Assuan server protocol implementation.
//!
//! Implements the server side of the GnuPG Assuan IPC protocol used by
//! pinentry programs. Parses commands from gpg-agent on stdin and sends
//! responses on stdout.

use std::io::{BufRead, Write};

use zeroize::Zeroize;

use crate::state::PinentryState;

// GPG error codes (source=1 GPG_ERR_SOURCE_UNKNOWN | code).
pub const GPG_ERR_CANCELED: u32 = 83886179;
pub const GPG_ERR_NOT_CONFIRMED: u32 = 83886194;
pub const GPG_ERR_ASS_UNKNOWN_CMD: u32 = 83886381;

/// Result returned by the handler callback for GETPIN/CONFIRM/MESSAGE.
pub enum HandlerResult {
    /// Passphrase obtained (GETPIN only). Auto-zeroized on drop.
    Pin(zeroize::Zeroizing<String>),
    /// User confirmed (CONFIRM/MESSAGE).
    Confirmed,
    /// User canceled the operation.
    Canceled,
    /// User chose "not ok" (CONFIRM only).
    NotConfirmed,
}

/// Percent-decode an Assuan parameter string.
///
/// Decodes `%XX` hex sequences per the Assuan spec. Works at the byte level
/// so that multi-byte UTF-8 sequences encoded as `%C3%BC` etc. decode
/// correctly. Malformed or truncated sequences are passed through literally.
fn percent_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.as_bytes().iter().copied();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            match (bytes.next(), bytes.next()) {
                (Some(h1), Some(h2)) => {
                    let hex = [h1, h2];
                    // hex digits are ASCII, so from_utf8 is infallible
                    let hex_str = std::str::from_utf8(&hex).unwrap_or("");
                    if let Ok(decoded) = u8::from_str_radix(hex_str, 16) {
                        out.push(decoded);
                    } else {
                        out.push(b'%');
                        out.extend_from_slice(&hex);
                    }
                }
                (Some(h1), None) => {
                    out.push(b'%');
                    out.push(h1);
                }
                _ => {
                    out.push(b'%');
                }
            }
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Percent-encode data for an Assuan `D` response line.
fn percent_encode_data(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '%' => out.push_str("%25"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            _ => out.push(c),
        }
    }
    out
}

/// Assuan protocol server, generic over I/O and handler.
///
/// The handler callback is invoked for GETPIN, CONFIRM, and MESSAGE commands
/// and must return a [`HandlerResult`].
pub struct Server<R, W, H> {
    reader: R,
    writer: W,
    handler: H,
    state: PinentryState,
}

impl<R: BufRead, W: Write, H> Server<R, W, H>
where
    H: FnMut(&PinentryState) -> HandlerResult,
{
    pub fn new(reader: R, writer: W, handler: H) -> Self {
        Self {
            reader,
            writer,
            handler,
            state: PinentryState::default(),
        }
    }

    fn send_ok(&mut self, msg: Option<&str>) -> std::io::Result<()> {
        match msg {
            Some(m) => writeln!(self.writer, "OK {m}"),
            None => writeln!(self.writer, "OK"),
        }
    }

    fn send_err(&mut self, code: u32, desc: &str) -> std::io::Result<()> {
        writeln!(self.writer, "ERR {code} {desc}")
    }

    // Assuan line limit: 1000 bytes including CRLF. "D " prefix is 2 bytes,
    // LF is 1 byte, leaving 997 bytes for encoded data.
    const MAX_CHUNK: usize = 997;

    /// Send a D (data) response. Splits into multiple D lines if the encoded
    /// data would exceed the Assuan 1000-byte line limit.
    fn send_data(&mut self, data: &str) -> std::io::Result<()> {
        let mut encoded = percent_encode_data(data);
        let mut remaining = encoded.as_str();
        while !remaining.is_empty() {
            let end = remaining.len().min(Self::MAX_CHUNK);
            writeln!(self.writer, "D {}", &remaining[..end])?;
            remaining = &remaining[end..];
        }
        encoded.zeroize();
        writeln!(self.writer, "OK")
    }

    fn handle_getpin(&mut self) -> std::io::Result<()> {
        self.state.set_cmd_getpin();
        match (self.handler)(&self.state) {
            HandlerResult::Pin(pin) => {
                self.send_data(&pin)?;
            }
            HandlerResult::Confirmed | HandlerResult::Canceled | HandlerResult::NotConfirmed => {
                self.send_err(GPG_ERR_CANCELED, "Operation cancelled")?;
            }
        }
        self.state.reset();
        Ok(())
    }

    fn handle_confirm(&mut self) -> std::io::Result<()> {
        self.state.set_cmd_confirm();
        match (self.handler)(&self.state) {
            HandlerResult::Confirmed => {
                self.send_ok(None)?;
            }
            HandlerResult::Canceled | HandlerResult::Pin(_) => {
                self.send_err(GPG_ERR_CANCELED, "Operation cancelled")?;
            }
            HandlerResult::NotConfirmed => {
                self.send_err(GPG_ERR_NOT_CONFIRMED, "Not confirmed")?;
            }
        }
        self.state.reset();
        Ok(())
    }

    fn handle_message(&mut self) -> std::io::Result<()> {
        self.state.set_cmd_message();
        match (self.handler)(&self.state) {
            HandlerResult::Confirmed | HandlerResult::Pin(_) | HandlerResult::NotConfirmed => {
                self.send_ok(None)?;
            }
            HandlerResult::Canceled => {
                self.send_err(GPG_ERR_CANCELED, "Operation cancelled")?;
            }
        }
        self.state.reset();
        Ok(())
    }

    fn handle_getinfo(&mut self, param: &str) -> std::io::Result<()> {
        match param {
            "pid" => self.send_data(&std::process::id().to_string()),
            "version" => self.send_data(env!("CARGO_PKG_VERSION")),
            "flavor" => self.send_data("zellij"),
            "ttyinfo" => self.send_data("- - -"),
            _ => self.send_err(GPG_ERR_ASS_UNKNOWN_CMD, "Unknown GETINFO subcommand"),
        }
    }

    fn process_line(&mut self, line: &str) -> std::io::Result<bool> {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return Ok(true);
        }

        // Assuan comment lines start with '#'
        if line.starts_with('#') {
            return Ok(true);
        }

        let (cmd, param) = match line.find(' ') {
            Some(pos) => (&line[..pos], line[pos + 1..].trim_start()),
            None => (line, ""),
        };

        if cmd.eq_ignore_ascii_case("SETDESC") {
            self.state.desc = Some(percent_decode(param));
            self.send_ok(None)?;
        } else if cmd.eq_ignore_ascii_case("SETPROMPT") {
            self.state.prompt = Some(percent_decode(param));
            self.send_ok(None)?;
        } else if cmd.eq_ignore_ascii_case("SETTITLE") {
            self.state.title = Some(percent_decode(param));
            self.send_ok(None)?;
        } else if cmd.eq_ignore_ascii_case("SETERROR") {
            self.state.error = Some(percent_decode(param));
            self.send_ok(None)?;
        } else if cmd.eq_ignore_ascii_case("SETOK") {
            self.state.ok = Some(percent_decode(param));
            self.send_ok(None)?;
        } else if cmd.eq_ignore_ascii_case("SETNOTOK") {
            self.state.notok = Some(percent_decode(param));
            self.send_ok(None)?;
        } else if cmd.eq_ignore_ascii_case("SETCANCEL") {
            self.state.cancel = Some(percent_decode(param));
            self.send_ok(None)?;
        } else if cmd.eq_ignore_ascii_case("SETREPEAT") {
            self.state.repeat = Some(percent_decode(param));
            self.send_ok(None)?;
        } else if cmd.eq_ignore_ascii_case("SETREPEATERROR") {
            self.state.repeat_error = Some(percent_decode(param));
            self.send_ok(None)?;
        } else if cmd.eq_ignore_ascii_case("SETTIMEOUT") {
            self.state.timeout = param.parse().ok();
            self.send_ok(None)?;
        } else if cmd.eq_ignore_ascii_case("SETQUALITYBAR") {
            self.state.quality_bar = true;
            self.send_ok(None)?;
        } else if cmd.eq_ignore_ascii_case("SETKEYINFO") {
            self.state.keyinfo = Some(param.to_string());
            self.send_ok(None)?;
        } else if cmd.eq_ignore_ascii_case("OPTION") {
            if let Some((key, val)) = param.split_once('=') {
                self.state.set_option(key.to_string(), val.to_string());
            } else {
                self.state.set_option(param.to_string(), String::new());
            }
            self.send_ok(None)?;
        } else if cmd.eq_ignore_ascii_case("GETPIN") {
            self.handle_getpin()?;
        } else if cmd.eq_ignore_ascii_case("CONFIRM") {
            self.handle_confirm()?;
        } else if cmd.eq_ignore_ascii_case("MESSAGE") {
            self.handle_message()?;
        } else if cmd.eq_ignore_ascii_case("GETINFO") {
            self.handle_getinfo(param)?;
        } else if cmd.eq_ignore_ascii_case("NOP") {
            self.send_ok(None)?;
        } else if cmd.eq_ignore_ascii_case("BYE") {
            self.send_ok(Some("closing connection"))?;
            return Ok(false);
        } else {
            self.send_err(GPG_ERR_ASS_UNKNOWN_CMD, "Unknown command")?;
        }
        Ok(true)
    }

    /// Run the server loop. Sends initial OK greeting then processes commands
    /// until BYE or EOF.
    pub fn run(&mut self) -> std::io::Result<()> {
        self.send_ok(Some("Pleased to meet you"))?;
        self.writer.flush()?;

        let mut line = String::new();
        loop {
            line.clear();
            let n = self.reader.read_line(&mut line)?;
            if n == 0 {
                break;
            }
            if !self.process_line(&line)? {
                break;
            }
            self.writer.flush()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn pin(s: &str) -> HandlerResult {
        HandlerResult::Pin(zeroize::Zeroizing::new(s.into()))
    }

    fn run_conversation(
        input: &str,
        handler: impl FnMut(&PinentryState) -> HandlerResult,
    ) -> String {
        let reader = Cursor::new(input.as_bytes().to_vec());
        let mut output = Vec::new();
        {
            let mut server = Server::new(reader, &mut output, handler);
            server.run().unwrap();
        }
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn greeting_and_bye() {
        let output = run_conversation("BYE\n", |_| HandlerResult::Canceled);
        assert_eq!(output, "OK Pleased to meet you\nOK closing connection\n");
    }

    #[test]
    fn nop() {
        let output = run_conversation("NOP\nBYE\n", |_| HandlerResult::Canceled);
        assert_eq!(
            output,
            "OK Pleased to meet you\nOK\nOK closing connection\n"
        );
    }

    #[test]
    fn unknown_command() {
        let output = run_conversation("FOOBAR\nBYE\n", |_| HandlerResult::Canceled);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "OK Pleased to meet you");
        assert!(lines[1].starts_with("ERR 83886381"));
        assert_eq!(lines[2], "OK closing connection");
    }

    #[test]
    fn setdesc_and_getpin_ok() {
        let output = run_conversation(
            "SETDESC Please enter your passphrase\nGETPIN\nBYE\n",
            |state| {
                assert_eq!(state.desc.as_deref(), Some("Please enter your passphrase"));
                pin("mysecret")
            },
        );
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "OK Pleased to meet you");
        assert_eq!(lines[1], "OK"); // SETDESC
        assert_eq!(lines[2], "D mysecret"); // GETPIN data
        assert_eq!(lines[3], "OK"); // GETPIN ok
        assert_eq!(lines[4], "OK closing connection");
    }

    #[test]
    fn getpin_canceled() {
        let output = run_conversation("GETPIN\nBYE\n", |_| HandlerResult::Canceled);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "OK Pleased to meet you");
        assert!(lines[1].starts_with("ERR 83886179"));
    }

    #[test]
    fn confirm_ok() {
        let output = run_conversation("SETDESC Do you trust this key?\nCONFIRM\nBYE\n", |_| {
            HandlerResult::Confirmed
        });
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "OK Pleased to meet you");
        assert_eq!(lines[1], "OK"); // SETDESC
        assert_eq!(lines[2], "OK"); // CONFIRM
        assert_eq!(lines[3], "OK closing connection");
    }

    #[test]
    fn confirm_not_confirmed() {
        let output = run_conversation("CONFIRM\nBYE\n", |_| HandlerResult::NotConfirmed);
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[1].starts_with("ERR 83886194"));
    }

    #[test]
    fn confirm_canceled() {
        let output = run_conversation("CONFIRM\nBYE\n", |_| HandlerResult::Canceled);
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[1].starts_with("ERR 83886179"));
    }

    #[test]
    fn message_ok() {
        let output = run_conversation("SETDESC Info message\nMESSAGE\nBYE\n", |_| {
            HandlerResult::Confirmed
        });
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[2], "OK"); // MESSAGE
    }

    #[test]
    fn message_canceled() {
        let output = run_conversation("MESSAGE\nBYE\n", |_| HandlerResult::Canceled);
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[1].starts_with("ERR 83886179"));
    }

    #[test]
    fn percent_decoding() {
        let output = run_conversation("SETDESC line1%0Aline2%25done\nGETPIN\nBYE\n", |state| {
            assert_eq!(state.desc.as_deref(), Some("line1\nline2%done"));
            pin("x")
        });
        assert!(output.contains("D x\n"));
    }

    #[test]
    fn percent_encoding_in_data() {
        let output = run_conversation("GETPIN\nBYE\n", |_| pin("pass%word\nnewline"));
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[1], "D pass%25word%0Anewline");
    }

    #[test]
    fn state_resets_after_getpin() {
        let call_count = std::cell::Cell::new(0);
        let output = run_conversation("SETDESC first\nGETPIN\nGETPIN\nBYE\n", |state| {
            let n = call_count.get();
            call_count.set(n + 1);
            if n == 0 {
                assert_eq!(state.desc.as_deref(), Some("first"));
            } else {
                assert_eq!(state.desc, None);
            }
            pin("p")
        });
        assert!(output.contains("OK closing connection"));
    }

    #[test]
    fn state_resets_after_confirm() {
        let call_count = std::cell::Cell::new(0);
        let output = run_conversation("SETDESC first\nCONFIRM\nCONFIRM\nBYE\n", |state| {
            let n = call_count.get();
            call_count.set(n + 1);
            if n == 0 {
                assert_eq!(state.desc.as_deref(), Some("first"));
            } else {
                assert_eq!(state.desc, None);
            }
            HandlerResult::Confirmed
        });
        assert!(output.contains("OK closing connection"));
    }

    #[test]
    fn multiple_setters() {
        let output = run_conversation(
            "SETTITLE My Title\nSETDESC My Desc\nSETPROMPT Pass:\nSETERROR Bad pin\nSETOK Yes\nSETNOTOK No\nSETCANCEL Abort\nSETREPEAT Again:\nSETREPEATERROR Mismatch\nSETKEYINFO key123\nSETQUALITYBAR\nSETTIMEOUT 30\nGETPIN\nBYE\n",
            |state| {
                assert_eq!(state.title.as_deref(), Some("My Title"));
                assert_eq!(state.desc.as_deref(), Some("My Desc"));
                assert_eq!(state.prompt.as_deref(), Some("Pass:"));
                assert_eq!(state.error.as_deref(), Some("Bad pin"));
                assert_eq!(state.ok.as_deref(), Some("Yes"));
                assert_eq!(state.notok.as_deref(), Some("No"));
                assert_eq!(state.cancel.as_deref(), Some("Abort"));
                assert_eq!(state.repeat.as_deref(), Some("Again:"));
                assert_eq!(state.repeat_error.as_deref(), Some("Mismatch"));
                assert_eq!(state.keyinfo.as_deref(), Some("key123"));
                assert!(state.quality_bar);
                assert_eq!(state.timeout, Some(30));
                pin("ok")
            },
        );
        assert!(output.contains("D ok\n"));
    }

    #[test]
    fn option_command() {
        let output = run_conversation(
            "OPTION allow-external-password-cache\nOPTION ttyname=/dev/pts/1\nGETPIN\nBYE\n",
            |state| {
                assert_eq!(
                    state
                        .options
                        .get("allow-external-password-cache")
                        .map(String::as_str),
                    Some("")
                );
                assert_eq!(
                    state.options.get("ttyname").map(String::as_str),
                    Some("/dev/pts/1")
                );
                pin("p")
            },
        );
        assert!(output.contains("D p\n"));
    }

    #[test]
    fn getinfo_pid() {
        let output = run_conversation("GETINFO pid\nBYE\n", |_| HandlerResult::Canceled);
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[1].starts_with("D "));
        let pid_str = lines[1].strip_prefix("D ").unwrap();
        assert!(pid_str.parse::<u32>().is_ok());
    }

    #[test]
    fn getinfo_version() {
        let output = run_conversation("GETINFO version\nBYE\n", |_| HandlerResult::Canceled);
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[1].starts_with("D "));
    }

    #[test]
    fn getinfo_flavor() {
        let output = run_conversation("GETINFO flavor\nBYE\n", |_| HandlerResult::Canceled);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[1], "D zellij");
    }

    #[test]
    fn getinfo_ttyinfo() {
        let output = run_conversation("GETINFO ttyinfo\nBYE\n", |_| HandlerResult::Canceled);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[1], "D - - -");
    }

    #[test]
    fn getinfo_unknown() {
        let output = run_conversation("GETINFO bogus\nBYE\n", |_| HandlerResult::Canceled);
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[1].starts_with("ERR"));
    }

    #[test]
    fn comment_line_ignored() {
        let output = run_conversation("# this is a comment\nNOP\nBYE\n", |_| {
            HandlerResult::Canceled
        });
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "OK Pleased to meet you");
        assert_eq!(lines[1], "OK"); // NOP
        assert_eq!(lines[2], "OK closing connection");
    }

    #[test]
    fn comment_without_space() {
        let output = run_conversation("#comment\nNOP\nBYE\n", |_| HandlerResult::Canceled);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "OK Pleased to meet you");
        assert_eq!(lines[1], "OK"); // NOP
    }

    #[test]
    fn empty_line_ignored() {
        let output = run_conversation("\nNOP\nBYE\n", |_| HandlerResult::Canceled);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "OK Pleased to meet you");
        assert_eq!(lines[1], "OK");
        assert_eq!(lines[2], "OK closing connection");
    }

    #[test]
    fn eof_terminates() {
        let output = run_conversation("NOP\n", |_| HandlerResult::Canceled);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "OK Pleased to meet you");
        assert_eq!(lines[1], "OK");
    }

    #[test]
    fn case_insensitive_commands() {
        let output = run_conversation("setdesc hello\ngetpin\nbye\n", |state| {
            assert_eq!(state.desc.as_deref(), Some("hello"));
            pin("p")
        });
        assert!(output.contains("D p\n"));
        assert!(output.contains("OK closing connection"));
    }

    #[test]
    fn percent_decode_edge_cases() {
        // Truncated percent sequence
        assert_eq!(percent_decode("abc%2"), "abc%2");
        assert_eq!(percent_decode("abc%"), "abc%");
        // Invalid hex
        assert_eq!(percent_decode("%ZZ"), "%ZZ");
        // Multiple sequences
        assert_eq!(percent_decode("%25%0A%0D"), "%\n\r");
    }

    #[test]
    fn crlf_line_endings() {
        let output = run_conversation("NOP\r\nBYE\r\n", |_| HandlerResult::Canceled);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "OK Pleased to meet you");
        assert_eq!(lines[1], "OK");
        assert_eq!(lines[2], "OK closing connection");
    }

    #[test]
    fn getpin_returns_confirmed_treated_as_cancel() {
        let output = run_conversation("GETPIN\nBYE\n", |_| HandlerResult::Confirmed);
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[1].starts_with("ERR 83886179"));
    }

    #[test]
    fn confirm_returns_pin_treated_as_cancel() {
        let output = run_conversation("CONFIRM\nBYE\n", |_| pin("x"));
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[1].starts_with("ERR 83886179"));
    }

    #[test]
    fn percent_decode_multibyte_utf8() {
        // %C3%BC = UTF-8 for 'ü' (U+00FC)
        assert_eq!(percent_decode("%C3%BC"), "ü");
        // %E2%9C%93 = UTF-8 for '✓' (U+2713)
        assert_eq!(percent_decode("%E2%9C%93"), "✓");
        // Mixed literal and encoded
        assert_eq!(percent_decode("hello%C3%BCworld"), "helloüworld");
    }

    #[test]
    fn long_data_split_into_multiple_d_lines() {
        // Create a passphrase longer than 997 bytes to trigger line splitting
        let long_pin = "A".repeat(2000);
        let output = run_conversation("GETPIN\nBYE\n", |_| {
            HandlerResult::Pin(zeroize::Zeroizing::new(long_pin.clone()))
        });
        let d_lines: Vec<&str> = output.lines().filter(|l| l.starts_with("D ")).collect();
        // Should be split into multiple D lines (2000 bytes > 997)
        assert!(
            d_lines.len() >= 2,
            "expected multiple D lines, got {}",
            d_lines.len()
        );
        // Reassembled data should equal the original
        let reassembled: String = d_lines
            .iter()
            .map(|l| l.strip_prefix("D ").unwrap())
            .collect();
        assert_eq!(reassembled, long_pin);
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #![proptest_config(proptest::prelude::ProptestConfig::with_cases(50000))]

            #[test]
            fn percent_decode_never_panics(s in ".*") {
                let _ = percent_decode(&s);
            }

            #[test]
            fn percent_decode_roundtrip(s in "[a-zA-Z0-9 .,!?@#$%^&*()]+") {
                // Encode then decode should roundtrip for printable ASCII
                let encoded = percent_encode_data(&s);
                let decoded = percent_decode(&encoded);
                prop_assert_eq!(decoded, s);
            }

            #[test]
            fn percent_decode_preserves_ascii(s in "[a-zA-Z0-9 ]+") {
                // Pure ASCII without % should pass through unchanged
                let decoded = percent_decode(&s);
                prop_assert_eq!(decoded, s);
            }
        }
    }
}
