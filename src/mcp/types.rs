use schemars::JsonSchema;
use serde::Deserialize;

macro_rules! params {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $(pub $field: $ty),* }
    };
}
params!(List { scope: Option<Scope>, cursor: Option<String>, limit: Option<u32> });
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    All,
    Mine,
    Available,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Desktop,
    Terminal,
}
params!(Open { operation_id: Option<String>, peer_id: String, password: Option<String>, kind: Option<Kind>, force_relay: Option<bool>, wait_ms: Option<u64> });
params!(Attach { session_id: String, operation_id: Option<String> });
params!(Write { session_ref: String, operation_id: Option<String> });
params!(Read {
    session_ref: String
});
params!(Get { session_ref: String, detail: Option<Detail>, after_revision: Option<String>, wait_ms: Option<u64>, operation_id: Option<String> });
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    Summary,
    Full,
}
params!(Authenticate { session_ref: String, operation_id: Option<String>, challenge_id: String, credentials: Credentials, wait_ms: Option<u64> });
#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Credentials {
    Password { password: String },
    TwoFactor { code: String },
    OsLogin { username: String, password: String },
}
params!(Reconnect { session_ref: String, operation_id: Option<String>, force_relay: Option<bool>, wait_ms: Option<u64> });
params!(ControlRequest { session_ref: String, operation_id: Option<String>, reason: Option<String>, wait_ms: Option<u64> });
params!(ControlCancel { session_ref: String, operation_id: Option<String>, approval_id: String });
params!(Capture { session_ref: String, display_id: Option<String>, after_frame_seq: Option<String>, wait_ms: Option<u64>, max_width: Option<u32>, max_height: Option<u32> });
params!(CaptureOptions { display_id: Option<String>, wait_ms: Option<u64>, max_width: Option<u32>, max_height: Option<u32> });
params!(Input { session_ref: String, operation_id: Option<String>, actions: Vec<crate::automation::input::Action>, snapshot_id: Option<String>, capture: Option<CaptureOptions> });
params!(TerminalCreate { session_ref: String, operation_id: Option<String>, rows: Option<u32>, cols: Option<u32>, wait_ms: Option<u64> });
params!(TerminalRead { session_ref: String, terminal_id: String, cursor: Option<String>, format: Option<OutputFormat>, max_bytes: Option<usize>, wait_ms: Option<u64> });
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Text,
    Base64,
    Both,
}
impl OutputFormat {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Base64 => "base64",
            Self::Both => "both",
        }
    }
}
params!(TerminalReadOptions { cursor: Option<String>, format: Option<OutputFormat>, max_bytes: Option<usize>, wait_ms: Option<u64> });
params!(TerminalWrite { session_ref: String, operation_id: Option<String>, terminal_id: String, text: String, read: Option<TerminalReadOptions> });
params!(TerminalResize { session_ref: String, operation_id: Option<String>, terminal_id: String, rows: u32, cols: u32 });
params!(TerminalClose { session_ref: String, operation_id: Option<String>, terminal_id: String });

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn action_variants_and_tool_parameters_reject_unknown_fields() {
        assert!(serde_json::from_str::<Input>(
            r#"{"session_ref":"r","actions":[{"type":"key_down","key":"KeyA","x":2}]}"#
        )
        .is_err());
        assert!(serde_json::from_str::<Open>(r#"{"peer_id":"123","agent_id":"spoof"}"#).is_err());
        assert!(serde_json::from_str::<Credentials>(
            r#"{"kind":"password","password":"x","username":"y"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<Input>(
            r#"{"session_ref":"r","actions":[{"type":"key_down","key":"KeyA"}]}"#
        )
        .is_ok());
    }
}
