use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BridgeError {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    pub retry: &'static str,
}

impl BridgeError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            details: None,
            message: message.into(),
            retry: "after_state_change",
        }
    }
    pub fn details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "INVALID_ARGUMENT",
            details: None,
            message: message.into(),
            retry: "never",
        }
    }
}

pub type Result<T> = std::result::Result<T, BridgeError>;
