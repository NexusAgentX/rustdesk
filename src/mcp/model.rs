use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{future::Future, pin::Pin};
use tokio_util::sync::CancellationToken;

pub const LEGACY_VERSION: &str = "2025-11-25";
pub const CURRENT_VERSION: &str = "2026-07-28";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolVersion {
    Legacy,
    Current,
}

impl ProtocolVersion {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => LEGACY_VERSION,
            Self::Current => CURRENT_VERSION,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Implementation {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    pub read_only_hint: bool,
    pub destructive_hint: bool,
    pub idempotent_hint: bool,
    pub open_world_hint: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Content {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

impl Content {
    pub fn image(bytes: &[u8], mime_type: &str) -> Self {
        Self::Image {
            data: STANDARD.encode(bytes),
            mime_type: mime_type.to_owned(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub content: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Map<String, Value>>,
    pub is_error: bool,
}

impl ToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text { text: text.into() }],
            structured_content: None,
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            is_error: true,
            ..Self::text(text)
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, message)
    }

    pub(crate) fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

#[derive(Clone)]
pub struct CallContext {
    pub principal: String,
    /// Bind business handles to this credential generation as well as the principal.
    pub credential: String,
    pub protocol_version: ProtocolVersion,
    pub cancellation: CancellationToken,
}

pub type ToolFuture = Pin<Box<dyn Future<Output = Result<ToolResult, RpcError>> + Send>>;

/// Constructing the future must not perform side effects.
/// Implementations validate arguments and check business authorization at the point of effect.
/// Dropping the future or cancelling its token must stop queued work and release held input.
pub trait ToolHandler: Send + Sync + 'static {
    fn call(&self, context: CallContext, arguments: Map<String, Value>) -> ToolFuture;
}
