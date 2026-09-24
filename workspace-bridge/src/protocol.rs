use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

pub mod error_codes {
    pub const UNAUTHENTICATED: &str = "UNAUTHENTICATED";
    pub const PROTOCOL_MISMATCH: &str = "PROTOCOL_MISMATCH";
    pub const WORKSPACE_NOT_AUTHORIZED: &str = "WORKSPACE_NOT_AUTHORIZED";
    pub const PATH_OUTSIDE_WORKSPACE: &str = "PATH_OUTSIDE_WORKSPACE";
    pub const NOT_FOUND: &str = "NOT_FOUND";
    pub const NOT_TEXT_FILE: &str = "NOT_TEXT_FILE";
    pub const FILE_TOO_LARGE: &str = "FILE_TOO_LARGE";
    pub const REVISION_CONFLICT: &str = "REVISION_CONFLICT";
    pub const PATCH_REJECTED: &str = "PATCH_REJECTED";
    pub const COMMAND_NOT_ALLOWED: &str = "COMMAND_NOT_ALLOWED";
    pub const COMMAND_TIMEOUT: &str = "COMMAND_TIMEOUT";
    pub const RESULT_TRUNCATED: &str = "RESULT_TRUNCATED";
    pub const INTERNAL_ERROR: &str = "INTERNAL_ERROR";
}

/// Incoming JSON-RPC request envelope
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcRequest {
    pub v: u32,
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// Outgoing JSON-RPC response envelope
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcResponse {
    pub v: u32,
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    pub fn success(id: String, result: serde_json::Value) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(
        id: String,
        code: impl Into<String>,
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id,
            ok: false,
            result: None,
            error: Some(RpcError {
                code: code.into(),
                message: message.into(),
                data,
            }),
        }
    }
}

/// Standardized error object
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Server-push event envelope
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcEvent {
    pub v: u32,
    pub event: String,
    pub data: serde_json::Value,
}

impl RpcEvent {
    pub fn new(event: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            event: event.into(),
            data,
        }
    }
}

// ---------------- Method Parameter and Result Structs ---------------- //

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthHandshakeParams {
    pub token: String,
    #[serde(default)]
    pub client: Option<String>,
    #[serde(rename = "extensionId", alias = "extension_id", default)]
    pub extension_id: Option<String>,
    #[serde(rename = "protocolVersion", alias = "protocol_version", default)]
    pub protocol_version: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthHandshakeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: u32,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub root: String,
    #[serde(rename = "rootName")]
    pub root_name: String,
    pub capabilities: Vec<String>,
    pub git: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceStatusResult {
    pub connected: bool,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub root: String,
    #[serde(rename = "rootName")]
    pub root_name: String,
    pub read: bool,
    pub write: bool,
    pub command: bool,
    #[serde(rename = "watcherHealthy")]
    pub watcher_healthy: bool,
    pub git: bool,
    #[serde(rename = "openedFilesCount")]
    pub opened_files_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchCodeParams {
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(rename = "isRegex", alias = "is_regex", default)]
    pub is_regex: Option<bool>,
    #[serde(rename = "caseSensitive", alias = "case_sensitive", default)]
    pub case_sensitive: Option<bool>,
    #[serde(default)]
    pub include: Option<Vec<String>>,
    #[serde(default)]
    pub exclude: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMatch {
    pub path: String,
    pub line: usize,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchCodeResult {
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
    #[serde(rename = "totalMatches")]
    pub total_matches: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileParams {
    pub path: String,
    #[serde(rename = "maxChars", alias = "max_chars", default)]
    pub max_chars: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileResult {
    pub path: String,
    pub revision: u64,
    pub hash: String,
    pub content: String,
    pub size: usize,
    #[serde(rename = "totalLines")]
    pub total_lines: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadRangeParams {
    pub path: String,
    #[serde(rename = "startLine", alias = "start_line")]
    pub start_line: usize,
    #[serde(rename = "endLine", alias = "end_line")]
    pub end_line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadRangeResult {
    pub path: String,
    pub revision: u64,
    pub hash: String,
    pub content: String,
    #[serde(rename = "startLine")]
    pub start_line: usize,
    #[serde(rename = "endLine")]
    pub end_line: usize,
    #[serde(rename = "totalLines")]
    pub total_lines: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyPatchParams {
    pub path: String,
    #[serde(rename = "expectedRevision", alias = "expected_revision")]
    pub expected_revision: u64,
    #[serde(rename = "expectedHash", alias = "expected_hash", default)]
    pub expected_hash: Option<String>,
    #[serde(default)]
    pub patch: Option<String>,
    #[serde(rename = "oldText", alias = "old_text", default)]
    pub old_text: Option<String>,
    #[serde(rename = "newText", alias = "new_text", default)]
    pub new_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyPatchResult {
    pub path: String,
    #[serde(rename = "oldRevision")]
    pub old_revision: u64,
    #[serde(rename = "newRevision")]
    pub new_revision: u64,
    #[serde(rename = "newHash")]
    pub new_hash: String,
    #[serde(rename = "diffSummary")]
    pub diff_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFileParams {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub overwrite: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFileResult {
    pub path: String,
    pub revision: u64,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiffParams {
    #[serde(default)]
    pub paths: Option<Vec<String>>,
    #[serde(rename = "maxBytes", alias = "max_bytes", default)]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiffResult {
    pub diff: String,
    #[serde(rename = "filesChanged")]
    pub files_changed: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunCommandParams {
    pub command: String,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(rename = "timeoutMs", alias = "timeout_ms", default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunCommandResult {
    #[serde(rename = "exitCode")]
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    #[serde(rename = "timedOut")]
    pub timed_out: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEventData {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(rename = "oldPath", skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_request_deserialization() {
        let json =
            r#"{"v":1,"id":"req_1","method":"workspace.search_code","params":{"query":"test"}}"#;
        let req: RpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.v, 1);
        assert_eq!(req.id, "req_1");
        assert_eq!(req.method, "workspace.search_code");
        assert!(req.params.is_some());
    }

    #[test]
    fn test_rpc_response_success() {
        let resp = RpcResponse::success("req_1".to_string(), serde_json::json!({"matches": []}));
        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(serialized.contains(r#""ok":true"#));
        assert!(serialized.contains(r#""v":1"#));
        assert!(serialized.contains(r#""matches":[]"#));
    }

    #[test]
    fn test_rpc_response_error() {
        let resp = RpcResponse::error(
            "req_2".to_string(),
            error_codes::REVISION_CONFLICT,
            "File changed externally",
            Some(serde_json::json!({"currentRevision": 4})),
        );
        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(serialized.contains(r#""ok":false"#));
        assert!(serialized.contains("REVISION_CONFLICT"));
        assert!(serialized.contains("currentRevision"));
    }

    #[test]
    fn test_rpc_event_serialization() {
        let event = RpcEvent::new(
            "file.changed",
            serde_json::json!({"path": "src/main.rs", "revision": 2}),
        );
        let serialized = serde_json::to_string(&event).unwrap();
        assert!(serialized.contains(r#""event":"file.changed""#));
        assert!(serialized.contains(r#""v":1"#));
    }
}
