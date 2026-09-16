use schemars::JsonSchema;
use serde::Deserialize;

macro_rules! params {
    ($name:ident { $( $(#[$attr:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $( $(#[$attr])* pub $field: $ty),* }
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
    FileTransfer,
}
params!(Open { operation_id: Option<String>, peer_id: String, password: Option<String>, from_session_ref: Option<String>, kind: Option<Kind>, force_relay: Option<bool>, wait_ms: Option<u64> });
params!(Attach { session_id: String, operation_id: Option<String> });
params!(Write { session_ref: String, operation_id: Option<String> });
params!(WaitWrite { session_ref: String, operation_id: Option<String>, wait_ms: Option<u64> });
params!(Read {
    session_ref: String
});
params!(OperationGet { operation_id: String, wait_ms: Option<u64> });
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
params!(Capture {
    session_ref: String,
    display_id: Option<String>,
    /// Only return frames newer than this sequence; requires an actual display_id.
    after_frame_seq: Option<String>,
    /// Maximum wait for a qualifying frame, 0..30000 ms (default 0).
    /// Returns immediately if one is cached; without after_frame_seq any cached frame qualifies.
    /// Does not delay capture or wait for the remote application to finish.
    wait_ms: Option<u64>,
    max_width: Option<u32>,
    max_height: Option<u32>,
});
params!(CaptureOptions {
    display_id: Option<String>,
    /// Delay before observing after input sending finishes, 0..30000 ms (default 0).
    /// Separate from wait_ms; does not guarantee remote input processing or application completion.
    delay_ms: Option<u64>,
    /// After delay_ms, wait up to 0..30000 ms (default 1000) for a frame newer than
    /// the frame recorded BEFORE input sending. Returns immediately if one is cached,
    /// including a frame received during sending or the delay. Does not wait for visual stability.
    /// Timeout returns unchanged without a PNG if a frame exists, otherwise NO_FRAME.
    wait_ms: Option<u64>,
    max_width: Option<u32>,
    max_height: Option<u32>,
});
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
    fn input_keys_and_capture_timing_are_explicit() {
        let _: Input = serde_json::from_value(serde_json::json!({"session_ref":"r", "actions":[{"type":"wait","duration_ms":500}]})).unwrap();
        let _: WaitWrite = serde_json::from_value(serde_json::json!({"session_ref":"r", "wait_ms":1000})).unwrap();
        for name in ["L", "l", "Return", "UnsupportedKey"] {
            let value = serde_json::json!({"session_ref":"r","actions":[{"type":"key_press","key":name}]});
            let error = serde_json::from_value::<Input>(value).err().unwrap().to_string();
            assert!(error.contains(name));
        }
        let input: Input = serde_json::from_value(serde_json::json!({
            "session_ref":"r", "actions":[{"type":"shortcut","modifiers":["Control"],"key":"KeyL"}],
            "capture":{"delay_ms":500,"wait_ms":1000}
        })).unwrap();
        let capture = input.capture.unwrap();
        assert_eq!(capture.delay_ms, Some(500));
        assert_eq!(capture.wait_ms, Some(1000));
        let schema = serde_json::to_string(&schemars::schema_for!(Input)).unwrap();
        assert!(schema.contains("KeyL"));
        assert!(schema.contains("delay_ms"));
        assert!(schema.contains("BEFORE input sending"));
    }
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

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileLocation { Local, Remote }
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection { Upload, Download }
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy { Ask, Overwrite, Skip }
params!(FileList { session_ref: String, path: String, location: FileLocation, include_hidden: Option<bool>, wait_ms: Option<u64> });
params!(TransferItem { source_path: String, destination_path: String });
params!(FileTransfer { session_ref: String, operation_id: Option<String>, direction: TransferDirection, items: Vec<TransferItem>, include_hidden: Option<bool>, conflict: Option<ConflictPolicy> });
params!(FileJobGet { session_ref: String, job_id: String, after_revision: Option<u64>, wait_ms: Option<u64>, offset: Option<usize>, limit: Option<usize> });
params!(FileJobWrite { session_ref: String, operation_id: Option<String>, job_id: String });
params!(FileConflict { session_ref: String, operation_id: Option<String>, job_id: String, overwrite: bool, apply_to_remaining: Option<bool> });
params!(ClipboardRead { session_ref: String, after_revision: Option<u64>, wait_ms: Option<u64> });
params!(ClipboardSet { session_ref: String, operation_id: Option<String>, enabled: bool });
params!(ClipboardWrite { session_ref: String, operation_id: Option<String>, text: String, paste: Option<bool>, delay_ms: Option<u64> });
params!(ClipboardType { session_ref: String, operation_id: Option<String>, text: Option<String> });
