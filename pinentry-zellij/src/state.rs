//! Mutable state accumulated from Assuan setter commands.
//!
//! gpg-agent sends SETDESC, SETPROMPT, etc. before each GETPIN/CONFIRM/MESSAGE.
//! This module collects those values so the handler has full context.

use std::collections::HashMap;

use crate::protocol::{PinentryCmd, PinentryRequest};

/// State accumulated from Assuan setter commands between action commands.
#[derive(Debug, Default, Clone)]
pub struct PinentryState {
    pub title: Option<String>,
    pub desc: Option<String>,
    pub prompt: Option<String>,
    pub error: Option<String>,
    pub ok: Option<String>,
    pub notok: Option<String>,
    pub cancel: Option<String>,
    pub repeat: Option<String>,
    pub repeat_error: Option<String>,
    pub keyinfo: Option<String>,
    pub quality_bar: bool,
    pub timeout: Option<u32>,
    pub options: HashMap<String, String>,
    cmd: Option<PinentryCmd>,
}

impl PinentryState {
    pub fn set_option(&mut self, key: String, val: String) {
        self.options.insert(key, val);
    }

    pub fn set_cmd_getpin(&mut self) {
        self.cmd = Some(PinentryCmd::GetPin);
    }

    pub fn set_cmd_confirm(&mut self) {
        self.cmd = Some(PinentryCmd::Confirm);
    }

    pub fn set_cmd_message(&mut self) {
        self.cmd = Some(PinentryCmd::Message);
    }

    pub fn cmd(&self) -> Option<&PinentryCmd> {
        self.cmd.as_ref()
    }

    /// Convert the accumulated state into a serializable request.
    pub fn to_request(&self) -> PinentryRequest {
        PinentryRequest {
            cmd: self.cmd.clone().unwrap_or(PinentryCmd::GetPin),
            title: self.title.clone(),
            desc: self.desc.clone(),
            prompt: self.prompt.clone(),
            error: self.error.clone(),
            ok: self.ok.clone(),
            cancel: self.cancel.clone(),
            notok: self.notok.clone(),
            repeat: self.repeat.clone(),
            repeat_error: self.repeat_error.clone(),
        }
    }

    /// Reset setter state after a GETPIN/CONFIRM/MESSAGE command.
    /// Options are preserved (they persist across commands per the Assuan protocol).
    pub fn reset(&mut self) {
        self.title = None;
        self.desc = None;
        self.prompt = None;
        self.error = None;
        self.ok = None;
        self.notok = None;
        self.cancel = None;
        self.repeat = None;
        self.repeat_error = None;
        self.keyinfo = None;
        self.quality_bar = false;
        self.timeout = None;
        self.cmd = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state() {
        let state = PinentryState::default();
        assert!(state.desc.is_none());
        assert!(state.title.is_none());
        assert!(!state.quality_bar);
        assert!(state.options.is_empty());
    }

    #[test]
    fn accumulation() {
        let mut state = PinentryState {
            title: Some("Title".into()),
            desc: Some("Desc".into()),
            prompt: Some("Pass:".into()),
            error: Some("Wrong".into()),
            ok: Some("OK".into()),
            cancel: Some("Cancel".into()),
            notok: Some("No".into()),
            repeat: Some("Again:".into()),
            repeat_error: Some("Mismatch".into()),
            keyinfo: Some("key1".into()),
            quality_bar: true,
            timeout: Some(60),
            ..Default::default()
        };
        state.set_option("ttyname".into(), "/dev/pts/0".into());

        assert_eq!(state.title.as_deref(), Some("Title"));
        assert_eq!(state.desc.as_deref(), Some("Desc"));
        assert_eq!(state.timeout, Some(60));
    }

    #[test]
    fn to_request_getpin() {
        let state = PinentryState {
            desc: Some("Enter passphrase".into()),
            prompt: Some("PIN:".into()),
            cmd: Some(PinentryCmd::GetPin),
            ..Default::default()
        };

        let req = state.to_request();
        assert_eq!(req.cmd, PinentryCmd::GetPin);
        assert_eq!(req.desc.as_deref(), Some("Enter passphrase"));
        assert_eq!(req.prompt.as_deref(), Some("PIN:"));
    }

    #[test]
    fn to_request_confirm() {
        let state = PinentryState {
            desc: Some("Trust this key?".into()),
            cmd: Some(PinentryCmd::Confirm),
            ..Default::default()
        };

        let req = state.to_request();
        assert_eq!(req.cmd, PinentryCmd::Confirm);
    }

    #[test]
    fn to_request_message() {
        let state = PinentryState {
            desc: Some("Info".into()),
            cmd: Some(PinentryCmd::Message),
            ..Default::default()
        };

        let req = state.to_request();
        assert_eq!(req.cmd, PinentryCmd::Message);
    }

    #[test]
    fn reset_clears_setters_but_keeps_options() {
        let mut state = PinentryState {
            title: Some("T".into()),
            desc: Some("D".into()),
            prompt: Some("P".into()),
            error: Some("E".into()),
            ok: Some("OK".into()),
            cancel: Some("C".into()),
            notok: Some("N".into()),
            repeat: Some("R".into()),
            repeat_error: Some("RE".into()),
            keyinfo: Some("K".into()),
            quality_bar: true,
            timeout: Some(30),
            cmd: Some(PinentryCmd::GetPin),
            ..Default::default()
        };
        state.set_option("ttyname".into(), "/dev/pts/0".into());

        state.reset();

        assert!(state.title.is_none());
        assert!(state.desc.is_none());
        assert!(state.prompt.is_none());
        assert!(state.error.is_none());
        assert!(state.ok.is_none());
        assert!(state.cancel.is_none());
        assert!(state.notok.is_none());
        assert!(state.repeat.is_none());
        assert!(state.repeat_error.is_none());
        assert!(state.keyinfo.is_none());
        assert!(!state.quality_bar);
        assert!(state.timeout.is_none());
        assert!(state.cmd().is_none());
        // Options persist
        assert_eq!(
            state.options.get("ttyname").map(String::as_str),
            Some("/dev/pts/0")
        );
    }
}
