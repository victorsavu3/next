use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── JSON-RPC 2.0 ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

impl JsonRpcResponse {
    pub fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Option<Value>, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
            }),
        }
    }

    pub fn method_not_found(id: Option<Value>, method: &str) -> Self {
        Self::err(id, -32601, format!("Method not found: {method}"))
    }

    pub fn parse_error() -> Self {
        Self::err(None, -32700, "Parse error")
    }

    pub fn invalid_params(id: Option<Value>, msg: impl Into<String>) -> Self {
        Self::err(id, -32602, msg)
    }
}

// ── MCP types ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ServerInfo {
    pub name: &'static str,
    pub version: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ServerCapabilities {
    pub tools: ToolsCapability,
}

#[derive(Debug, Serialize)]
pub struct ToolsCapability {
    #[serde(rename = "listChanged")]
    pub list_changed: bool,
}

#[derive(Debug, Serialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: &'static str,
    pub capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
    /// Free-text guidance the client MAY inject into the model's system prompt.
    /// Used to push tagging conventions and a snapshot of known contexts /
    /// resources onto the AI without it having to query for them first.
    /// Valid since protocol revision 2024-11-05.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

impl Default for InitializeResult {
    fn default() -> Self {
        Self::new()
    }
}

impl InitializeResult {
    pub fn new() -> Self {
        Self {
            protocol_version: "2024-11-05",
            capabilities: ServerCapabilities {
                tools: ToolsCapability {
                    list_changed: false,
                },
            },
            server_info: ServerInfo {
                name: "next-mcp",
                version: env!("CARGO_PKG_VERSION"),
            },
            instructions: None,
        }
    }

    /// Attach server instructions, consuming and returning `self` for chaining.
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Debug, Serialize)]
pub struct ToolsListResult {
    pub tools: Vec<Tool>,
}

#[derive(Debug, Serialize)]
pub struct CallToolResult {
    pub content: Vec<Content>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct Content {
    #[serde(rename = "type")]
    pub content_type: &'static str,
    pub text: String,
}

impl CallToolResult {
    pub fn success(value: Value) -> Self {
        Self {
            content: vec![Content {
                content_type: "text",
                text: serde_json::to_string_pretty(&value).unwrap_or_default(),
            }],
            is_error: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![Content {
                content_type: "text",
                text: message.into(),
            }],
            is_error: Some(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `instructions` is omitted from the wire when unset (default `new()`).
    #[test]
    fn initialize_omits_instructions_when_absent() {
        let v = serde_json::to_value(InitializeResult::new()).unwrap();
        assert!(
            v.get("instructions").is_none(),
            "instructions must be omitted: {v}"
        );
        assert_eq!(v["protocolVersion"], "2024-11-05");
    }

    /// `with_instructions` makes the field appear verbatim on the wire.
    #[test]
    fn initialize_serializes_instructions_when_present() {
        let v =
            serde_json::to_value(InitializeResult::new().with_instructions("hello guide")).unwrap();
        assert_eq!(v["instructions"], "hello guide");
    }
}
