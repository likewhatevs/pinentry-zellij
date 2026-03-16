//! JSON protocol types shared between the pinentry binary and the Zellij plugin.
//!
//! These types are serialized to JSON and passed over `zellij pipe` to
//! communicate the pinentry request to the plugin and receive its response.

use serde::{Deserialize, Serialize};

/// The type of pinentry operation requested.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PinentryCmd {
    GetPin,
    Confirm,
    Message,
}

/// Pinentry request sent from the binary to the plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PinentryRequest {
    pub cmd: PinentryCmd,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notok: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_error: Option<String>,
}

// Note: quality_bar and keyinfo are parsed from Assuan (in state.rs) but
// not sent to the plugin. quality_bar requires INQUIRE callback support
// to get a score from gpg-agent. keyinfo is for external password cache
// lookups, not display.

/// Status returned by the plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PinStatus {
    Ok,
    Canceled,
    NotConfirmed,
}

/// Response from the plugin back to the binary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PinResponse {
    pub status: PinStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trip() {
        let req = PinentryRequest {
            cmd: PinentryCmd::GetPin,
            title: Some("Key".into()),
            desc: Some("Enter passphrase".into()),
            prompt: Some("PIN:".into()),
            error: None,
            ok: Some("OK".into()),
            cancel: Some("Cancel".into()),
            notok: None,
            repeat: None,
            repeat_error: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: PinentryRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, decoded);
    }

    #[test]
    fn response_round_trip_ok() {
        let resp = PinResponse {
            status: PinStatus::Ok,
            passphrase: Some("secret".into()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: PinResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn response_round_trip_canceled() {
        let resp = PinResponse {
            status: PinStatus::Canceled,
            passphrase: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: PinResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn response_round_trip_not_confirmed() {
        let resp = PinResponse {
            status: PinStatus::NotConfirmed,
            passphrase: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: PinResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn request_omits_none_fields() {
        let req = PinentryRequest {
            cmd: PinentryCmd::Confirm,
            title: None,
            desc: Some("Confirm?".into()),
            prompt: None,
            error: None,
            ok: None,
            cancel: None,
            notok: None,
            repeat: None,
            repeat_error: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("\"title\""));
        assert!(!json.contains("\"prompt\""));
        assert!(json.contains("\"desc\""));
    }
}
