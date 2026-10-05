//! Control protocol: one JSON object per line over a Unix socket.
//!
//! ```text
//! → {"cmd":"switch","id":"win11"}
//! ← {"ok":true,"data":{"switched":{"from":"ubuntu","to":"win11","elapsed_us":412,...}}}
//! ```
//! The CLI, the overlay UI, and any script can drive Hyperswitch the same way.

use crate::switcher::{OsStatus, SwitchReport};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Status,
    Switch { id: String },
    Next,
    Prev,
    Start { id: String },
    Stop { id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Payload {
    Status(Vec<OsStatus>),
    Switched(SwitchReport),
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Payload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(p: Payload) -> Self {
        return Response {
            ok: true,
            data: Some(p),
            error: None,
        };
    }
    pub fn err(e: impl ToString) -> Self {
        return Response {
            ok: false,
            data: None,
            error: Some(e.to_string()),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_wire_format() {
        let r: Request = serde_json::from_str(r#"{"cmd":"switch","id":"win11"}"#).unwrap();
        assert_eq!(r, Request::Switch { id: "win11".into() });
        assert_eq!(
            serde_json::to_string(&Request::Next).unwrap(),
            r#"{"cmd":"next"}"#
        );
    }
}
