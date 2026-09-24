use crate::auth::AuthManager;
use crate::command::run_command;
use crate::files::{atomic_write, read_range, read_text_file, LineEnding};
use crate::git::{get_git_diff, is_git_repository};
use crate::patch::{apply_patch, PatchResult};
use crate::paths::{PathSandbox, SandboxError};
use crate::protocol::{
    error_codes, ApplyPatchParams, ApplyPatchResult, AuthHandshakeParams, AuthHandshakeResult,
    CreateFileParams, CreateFileResult, DirEntryInfo, GitDiffParams, GlobParams, GlobResult,
    ListDirParams, ListDirResult, ReadFileParams, ReadFileResult, ReadRangeParams, ReadRangeResult,
    RpcEvent, RpcRequest, RpcResponse, RunCommandParams, SearchCodeParams, WorkspaceStatusResult,
    PROTOCOL_VERSION,
};
use crate::search::search_code;
use crate::session::WorkspaceSession;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

/// Shared daemon state across WebSocket connections.
#[derive(Clone)]
pub struct ServerState {
    pub sandbox: Arc<PathSandbox>,
    pub auth: Arc<AuthManager>,
    pub session: Arc<WorkspaceSession>,
    pub event_tx: broadcast::Sender<RpcEvent>,
}

/// Run the workspace bridge WebSocket server on loopback.
pub async fn run_server(
    listener: TcpListener,
    state: ServerState,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    loop {
        tokio::select! {
            accept_res = listener.accept() => {
                match accept_res {
                    Ok((stream, addr)) => {
                        let client_state = state.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, addr, client_state).await {
                                eprintln!("[webbrain-workspace] Connection error from {addr}: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("[webbrain-workspace] TCP accept error: {e}");
                    }
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
        }
    }

    Ok(())
}
#[allow(clippy::result_large_err)]
async fn handle_connection(
    stream: TcpStream,
    _addr: SocketAddr,
    state: ServerState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut origin_rejected = false;

    // Inspect HTTP headers during WebSocket handshake for Origin validation
    let ws_stream = tokio_tungstenite::accept_hdr_async(stream, |req: &Request, resp: Response| {
        let origin_header = req.headers().get("Origin").and_then(|v| v.to_str().ok());

        if !state.auth.is_allowed_origin(origin_header) {
            origin_rejected = true;
        }

        Ok(resp)
    })
    .await?;

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    if origin_rejected {
        let frame = CloseFrame {
            code: CloseCode::Policy,
            reason: "Untrusted Origin: web origins are rejected".into(),
        };
        let _ = ws_tx.send(Message::Close(Some(frame))).await;
        return Ok(());
    }

    let mut authenticated = false;
    let mut event_rx = state.event_tx.subscribe();

    loop {
        tokio::select! {
            // Outgoing broadcast events pushed to this client
            event_res = event_rx.recv() => {
                if authenticated {
                    if let Ok(event) = event_res {
                        if let Ok(json_str) = serde_json::to_string(&event) {
                            if ws_tx.send(Message::Text(json_str.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }

            // Incoming messages from client
            msg_opt = ws_rx.next() => {
                let msg = match msg_opt {
                    Some(Ok(m)) => m,
                    Some(Err(_)) | None => break, // Client disconnected or error
                };

                match msg {
                    Message::Text(text) => {
                        let req: RpcRequest = match serde_json::from_str(&text) {
                            Ok(r) => r,
                            Err(e) => {
                                let err_resp = RpcResponse::error(
                                    "unknown".to_string(),
                                    error_codes::INTERNAL_ERROR,
                                    format!("Malformed JSON-RPC request: {e}"),
                                    None,
                                );
                                let _ = ws_tx.send(Message::Text(serde_json::to_string(&err_resp)?.into())).await;
                                continue;
                            }
                        };

                        let req_id = req.id.clone();

                        // Enforce authentication before any workspace command
                        if !authenticated {
                            if req.method == "auth.handshake" {
                                let resp = handle_handshake(&state, req, &mut authenticated);
                                let serialized = serde_json::to_string(&resp)?;
                                let _ = ws_tx.send(Message::Text(serialized.into())).await;
                                if !authenticated {
                                    // Close socket on failed handshake
                                    let frame = CloseFrame {
                                        code: CloseCode::Policy,
                                        reason: "Invalid authentication token".into(),
                                    };
                                    let _ = ws_tx.send(Message::Close(Some(frame))).await;
                                    break;
                                }
                            } else {
                                let resp = RpcResponse::error(
                                    req_id,
                                    error_codes::UNAUTHENTICATED,
                                    "auth.handshake is required before executing workspace operations",
                                    None,
                                );
                                let _ = ws_tx.send(Message::Text(serde_json::to_string(&resp)?.into())).await;
                            }
                            continue;
                        }

                        // Authenticated method dispatch
                        let resp = dispatch_method(&state, req).await;
                        let serialized = serde_json::to_string(&resp)?;
                        if ws_tx.send(Message::Text(serialized.into())).await.is_err() {
                            break;
                        }
                    }
                    Message::Ping(payload) => {
                        if ws_tx.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Message::Close(_) => {
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

fn handle_handshake(state: &ServerState, req: RpcRequest, authenticated: &mut bool) -> RpcResponse {
    let req_id = req.id;
    let params_val = match req.params {
        Some(p) => p,
        None => {
            return RpcResponse::error(
                req_id,
                error_codes::UNAUTHENTICATED,
                "auth.handshake requires parameters",
                None,
            );
        }
    };

    let params: AuthHandshakeParams = match serde_json::from_value(params_val) {
        Ok(p) => p,
        Err(e) => {
            return RpcResponse::error(
                req_id,
                error_codes::UNAUTHENTICATED,
                format!("Invalid handshake params: {e}"),
                None,
            );
        }
    };

    // Verify token
    if !state.auth.verify_token(&params.token) {
        return RpcResponse::error(
            req_id,
            error_codes::UNAUTHENTICATED,
            "Invalid pairing token",
            None,
        );
    }

    // Verify protocol version if specified
    if let Some(client_version) = params.protocol_version {
        if client_version > PROTOCOL_VERSION {
            return RpcResponse::error(
                req_id,
                error_codes::PROTOCOL_MISMATCH,
                format!(
                    "Unsupported protocol version {client_version}. Server supports v{PROTOCOL_VERSION}"
                ),
                None,
            );
        }
    }

    *authenticated = true;

    let mut capabilities = vec!["read".to_string()];
    if state.session.allow_write {
        capabilities.push("write".to_string());
    }
    if state.session.allow_command {
        capabilities.push("command".to_string());
    }

    let is_git = is_git_repository(state.sandbox.canonical_root());

    let result = AuthHandshakeResult {
        protocol_version: PROTOCOL_VERSION,
        session_id: state.session.session_id.clone(),
        root: state.sandbox.canonical_root().display().to_string(),
        root_name: state.session.root_name.clone(),
        capabilities,
        git: is_git,
    };

    RpcResponse::success(req_id, serde_json::to_value(result).unwrap())
}

async fn dispatch_method(state: &ServerState, req: RpcRequest) -> RpcResponse {
    let req_id = req.id;
    state.session.touch_activity();

    match req.method.as_str() {
        "workspace.status" => {
            let is_git = is_git_repository(state.sandbox.canonical_root());
            let root_display = state.sandbox.canonical_root().display().to_string();
            let root_name = state.session.root_name.clone();
            let guidance = format!(
                "The workspace root is '{}' ({}). Use '.' for the root directory itself. All relative paths start from this root.",
                root_name, root_display
            );
            let status = WorkspaceStatusResult {
                connected: true,
                session_id: state.session.session_id.clone(),
                root: root_display,
                root_name,
                guidance: Some(guidance),
                read: true,
                write: state.session.allow_write,
                command: state.session.allow_command,
                watcher_healthy: true,
                git: is_git,
                opened_files_count: state.session.opened_files_count(),
            };
            RpcResponse::success(req_id, serde_json::to_value(status).unwrap())
        }

        "workspace.search_code" => {
            let params: SearchCodeParams =
                match req.params.and_then(|p| serde_json::from_value(p).ok()) {
                    Some(p) => p,
                    None => {
                        return RpcResponse::error(
                            req_id,
                            error_codes::INTERNAL_ERROR,
                            "workspace.search_code requires valid parameters",
                            None,
                        );
                    }
                };

            match search_code(&state.sandbox, &params) {
                Ok(res) => RpcResponse::success(req_id, serde_json::to_value(res).unwrap()),
                Err(e) => {
                    RpcResponse::error(req_id, error_codes::INTERNAL_ERROR, e.to_string(), None)
                }
            }
        }

        "workspace.read_file" => {
            let params: ReadFileParams =
                match req.params.and_then(|p| serde_json::from_value(p).ok()) {
                    Some(p) => p,
                    None => {
                        return RpcResponse::error(
                            req_id,
                            error_codes::INTERNAL_ERROR,
                            "workspace.read_file requires valid parameters",
                            None,
                        );
                    }
                };

            let resolved = match state.sandbox.resolve(&params.path) {
                Ok(p) => p,
                Err(e) => return map_sandbox_error(req_id, e),
            };

            match read_text_file(&resolved, params.max_chars) {
                Ok(res) => {
                    let rel_path = state.sandbox.to_relative(&resolved).unwrap_or(params.path);
                    let mtime = std::fs::metadata(&resolved)
                        .and_then(|m| m.modified())
                        .unwrap_or_else(|_| std::time::SystemTime::now());

                    let rev = state
                        .session
                        .record_read(&rel_path, &res.hash, mtime, res.size);

                    let result = ReadFileResult {
                        path: rel_path,
                        revision: rev,
                        hash: res.hash,
                        content: res.content,
                        size: res.size,
                        total_lines: res.total_lines,
                        truncated: res.truncated,
                    };
                    RpcResponse::success(req_id, serde_json::to_value(result).unwrap())
                }
                Err(crate::files::FileError::NotFound(p)) => RpcResponse::error(
                    req_id,
                    error_codes::NOT_FOUND,
                    format!("File not found: {p}"),
                    None,
                ),
                Err(crate::files::FileError::NotTextFile(p)) => RpcResponse::error(
                    req_id,
                    error_codes::NOT_TEXT_FILE,
                    format!("Not a text file: {p}"),
                    None,
                ),
                Err(crate::files::FileError::FileTooLarge { size, limit }) => RpcResponse::error(
                    req_id,
                    error_codes::FILE_TOO_LARGE,
                    format!("File size {size} exceeds limit {limit}"),
                    None,
                ),
                Err(e) => {
                    RpcResponse::error(req_id, error_codes::INTERNAL_ERROR, e.to_string(), None)
                }
            }
        }

        "workspace.read_range" => {
            let params: ReadRangeParams =
                match req.params.and_then(|p| serde_json::from_value(p).ok()) {
                    Some(p) => p,
                    None => {
                        return RpcResponse::error(
                            req_id,
                            error_codes::INTERNAL_ERROR,
                            "workspace.read_range requires valid parameters",
                            None,
                        );
                    }
                };

            let resolved = match state.sandbox.resolve(&params.path) {
                Ok(p) => p,
                Err(e) => return map_sandbox_error(req_id, e),
            };

            match read_range(&resolved, params.start_line, params.end_line) {
                Ok((content, hash, total_lines)) => {
                    let rel_path = state.sandbox.to_relative(&resolved).unwrap_or(params.path);
                    let mtime = std::fs::metadata(&resolved)
                        .and_then(|m| m.modified())
                        .unwrap_or_else(|_| std::time::SystemTime::now());
                    let size = std::fs::metadata(&resolved)
                        .map(|m| m.len() as usize)
                        .unwrap_or(0);

                    let rev = state.session.record_read(&rel_path, &hash, mtime, size);

                    let result = ReadRangeResult {
                        path: rel_path,
                        revision: rev,
                        hash,
                        content,
                        start_line: params.start_line,
                        end_line: params.end_line,
                        total_lines,
                    };
                    RpcResponse::success(req_id, serde_json::to_value(result).unwrap())
                }
                Err(crate::files::FileError::NotFound(p)) => RpcResponse::error(
                    req_id,
                    error_codes::NOT_FOUND,
                    format!("File not found: {p}"),
                    None,
                ),
                Err(e) => {
                    RpcResponse::error(req_id, error_codes::INTERNAL_ERROR, e.to_string(), None)
                }
            }
        }

        "workspace.apply_patch" => {
            if !state.session.allow_write {
                return RpcResponse::error(
                    req_id,
                    error_codes::COMMAND_NOT_ALLOWED,
                    "Workspace editing is disabled. Enable --allow-write to make changes.",
                    None,
                );
            }

            let params: ApplyPatchParams =
                match req.params.and_then(|p| serde_json::from_value(p).ok()) {
                    Some(p) => p,
                    None => {
                        return RpcResponse::error(
                            req_id,
                            error_codes::INTERNAL_ERROR,
                            "workspace.apply_patch requires valid parameters",
                            None,
                        );
                    }
                };

            let resolved = match state.sandbox.resolve(&params.path) {
                Ok(p) => p,
                Err(e) => return map_sandbox_error(req_id, e),
            };

            let rel_path = state.sandbox.to_relative(&resolved).unwrap_or(params.path);

            if !resolved.exists() {
                if params.expected_revision > 1 {
                    return RpcResponse::error(
                        req_id,
                        error_codes::REVISION_CONFLICT,
                        format!(
                            "Revision conflict on {rel_path}: file does not exist but expected revision is {}",
                            params.expected_revision
                        ),
                        Some(serde_json::json!({
                            "expectedRevision": params.expected_revision,
                            "currentRevision": 0,
                        })),
                    );
                }

                let old_is_empty = params.old_text.as_deref().unwrap_or("").is_empty();
                if !old_is_empty && params.patch.is_none() {
                    return RpcResponse::error(
                        req_id,
                        error_codes::NOT_FOUND,
                        format!("File not found: {}", resolved.display()),
                        None,
                    );
                }

                let patch_res = if let Some(new_text) = params.new_text {
                    let lines_added = new_text.lines().count();
                    PatchResult {
                        new_content: new_text,
                        lines_added,
                        lines_removed: 0,
                        summary: format!("+{lines_added} / -0 lines in {rel_path}"),
                    }
                } else if let Some(patch_str) = params.patch.as_deref() {
                    match apply_patch("", None, None, Some(patch_str), &rel_path) {
                        Ok(pr) => pr,
                        Err(crate::patch::PatchError::ContextNotFound(msg)) => {
                            return RpcResponse::error(req_id, error_codes::PATCH_REJECTED, msg, None);
                        }
                        Err(crate::patch::PatchError::AmbiguousContext {
                            occurrences,
                            message,
                        }) => {
                            return RpcResponse::error(
                                req_id,
                                error_codes::PATCH_REJECTED,
                                format!("Ambiguous context ({occurrences} occurrences): {message}"),
                                Some(serde_json::json!({ "occurrences": occurrences })),
                            );
                        }
                        Err(e) => {
                            return RpcResponse::error(
                                req_id,
                                error_codes::PATCH_REJECTED,
                                e.to_string(),
                                None,
                            );
                        }
                    }
                } else {
                    PatchResult {
                        new_content: String::new(),
                        lines_added: 0,
                        lines_removed: 0,
                        summary: format!("+0 / -0 lines in {rel_path}"),
                    }
                };

                #[cfg(windows)]
                let default_line_ending = LineEnding::CrLf;
                #[cfg(not(windows))]
                let default_line_ending = LineEnding::Lf;

                let new_hash = match atomic_write(
                    &resolved,
                    &patch_res.new_content,
                    false,
                    Some(default_line_ending),
                ) {
                    Ok(h) => h,
                    Err(e) => {
                        return RpcResponse::error(
                            req_id,
                            error_codes::INTERNAL_ERROR,
                            format!("Write failed: {e}"),
                            None,
                        );
                    }
                };

                let new_mtime = std::fs::metadata(&resolved)
                    .and_then(|m| m.modified())
                    .unwrap_or_else(|_| std::time::SystemTime::now());
                let new_size = patch_res.new_content.len();

                let (old_rev, new_rev) =
                    state.session.commit_new_file(&rel_path, &new_hash, new_mtime, new_size);

                let result = ApplyPatchResult {
                    path: rel_path,
                    old_revision: old_rev,
                    new_revision: new_rev,
                    new_hash,
                    diff_summary: patch_res.summary,
                };

                return RpcResponse::success(req_id, serde_json::to_value(result).unwrap());
            }

            // Read current file
            let current_read = match read_text_file(&resolved, None) {
                Ok(r) => r,
                Err(crate::files::FileError::NotFound(p)) => {
                    return RpcResponse::error(
                        req_id,
                        error_codes::NOT_FOUND,
                        format!("File not found: {p}"),
                        None,
                    );
                }
                Err(e) => {
                    return RpcResponse::error(
                        req_id,
                        error_codes::INTERNAL_ERROR,
                        e.to_string(),
                        None,
                    );
                }
            };
            // Precondition: check expected revision & hash against current disk state before any modifications
            if let Err(crate::patch::PatchError::RevisionConflict {
                expected_revision,
                current_revision,
                expected_hash,
                current_hash,
            }) = state.session.check_revision(
                &rel_path,
                params.expected_revision,
                params.expected_hash.as_deref(),
                &current_read.hash,
            ) {
                return RpcResponse::error(
                    req_id,
                    error_codes::REVISION_CONFLICT,
                    format!(
                        "Revision conflict on {rel_path}: expected revision {expected_revision}, current is {current_revision}"
                    ),
                    Some(serde_json::json!({
                        "expectedRevision": expected_revision,
                        "currentRevision": current_revision,
                        "expectedHash": expected_hash,
                        "currentHash": current_hash,
                    })),
                );
            }

            // Apply patch to content
            let patch_res = match apply_patch(
                &current_read.content,
                params.old_text.as_deref(),
                params.new_text.as_deref(),
                params.patch.as_deref(),
                &rel_path,
            ) {
                Ok(pr) => pr,
                Err(crate::patch::PatchError::ContextNotFound(msg)) => {
                    return RpcResponse::error(req_id, error_codes::PATCH_REJECTED, msg, None);
                }
                Err(crate::patch::PatchError::AmbiguousContext {
                    occurrences,
                    message,
                }) => {
                    return RpcResponse::error(
                        req_id,
                        error_codes::PATCH_REJECTED,
                        format!("Ambiguous context ({occurrences} occurrences): {message}"),
                        Some(serde_json::json!({ "occurrences": occurrences })),
                    );
                }
                Err(e) => {
                    return RpcResponse::error(
                        req_id,
                        error_codes::PATCH_REJECTED,
                        e.to_string(),
                        None,
                    );
                }
            };

            // Write modified content atomically
            let new_hash = match atomic_write(
                &resolved,
                &patch_res.new_content,
                current_read.has_bom,
                Some(current_read.line_ending),
            ) {
                Ok(h) => h,
                Err(e) => {
                    return RpcResponse::error(
                        req_id,
                        error_codes::INTERNAL_ERROR,
                        format!("Write failed: {e}"),
                        None,
                    );
                }
            };

            let new_mtime = std::fs::metadata(&resolved)
                .and_then(|m| m.modified())
                .unwrap_or_else(|_| std::time::SystemTime::now());
            let new_size = patch_res.new_content.len();

            // Commit revision in session
            let (old_rev, new_rev) = state.session.commit_revision(
                &rel_path,
                &new_hash,
                new_mtime,
                new_size,
            );

            let result = ApplyPatchResult {
                path: rel_path,
                old_revision: old_rev,
                new_revision: new_rev,
                new_hash,
                diff_summary: patch_res.summary,
            };

            RpcResponse::success(req_id, serde_json::to_value(result).unwrap())
        }

        "workspace.create_file" => {
            if !state.session.allow_write {
                return RpcResponse::error(
                    req_id,
                    error_codes::COMMAND_NOT_ALLOWED,
                    "Workspace editing is disabled. Enable --allow-write to make changes.",
                    None,
                );
            }

            let params: CreateFileParams =
                match req.params.and_then(|p| serde_json::from_value(p).ok()) {
                    Some(p) => p,
                    None => {
                        return RpcResponse::error(
                            req_id,
                            error_codes::INTERNAL_ERROR,
                            "workspace.create_file requires valid parameters",
                            None,
                        );
                    }
                };

            let resolved = match state.sandbox.resolve(&params.path) {
                Ok(p) => p,
                Err(e) => return map_sandbox_error(req_id, e),
            };

            let rel_path = state.sandbox.to_relative(&resolved).unwrap_or(params.path);

            let file_exists = resolved.exists();
            if file_exists && params.overwrite != Some(true) {
                return RpcResponse::error(
                    req_id,
                    error_codes::INTERNAL_ERROR,
                    format!("File already exists: {rel_path}"),
                    None,
                );
            }

            #[cfg(windows)]
            let default_line_ending = LineEnding::CrLf;
            #[cfg(not(windows))]
            let default_line_ending = LineEnding::Lf;

            let new_hash = match atomic_write(
                &resolved,
                &params.content,
                false,
                Some(default_line_ending),
            ) {
                Ok(h) => h,
                Err(e) => {
                    return RpcResponse::error(
                        req_id,
                        error_codes::INTERNAL_ERROR,
                        format!("Write failed: {e}"),
                        None,
                    );
                }
            };

            let new_mtime = std::fs::metadata(&resolved)
                .and_then(|m| m.modified())
                .unwrap_or_else(|_| std::time::SystemTime::now());
            let new_size = params.content.len();

            let revision = if file_exists {
                let (_old_rev, new_rev) =
                    state.session.commit_revision(&rel_path, &new_hash, new_mtime, new_size);
                new_rev
            } else {
                let (_old_rev, new_rev) =
                    state.session.commit_new_file(&rel_path, &new_hash, new_mtime, new_size);
                new_rev
            };

            let result = CreateFileResult {
                path: rel_path,
                revision,
                hash: new_hash,
            };

            RpcResponse::success(req_id, serde_json::to_value(result).unwrap())
        }

        "workspace.git_diff" => {
            let params: GitDiffParams =
                match req.params.and_then(|p| serde_json::from_value(p).ok()) {
                    Some(p) => p,
                    None => GitDiffParams {
                        paths: None,
                        max_bytes: None,
                    },
                };

            match get_git_diff(&state.sandbox, &params) {
                Ok(res) => RpcResponse::success(req_id, serde_json::to_value(res).unwrap()),
                Err(e) => {
                    RpcResponse::error(req_id, error_codes::INTERNAL_ERROR, e.to_string(), None)
                }
            }
        }

        "workspace.run_command" => {
            let params: RunCommandParams =
                match req.params.and_then(|p| serde_json::from_value(p).ok()) {
                    Some(p) => p,
                    None => {
                        return RpcResponse::error(
                            req_id,
                            error_codes::INTERNAL_ERROR,
                            "workspace.run_command requires valid parameters",
                            None,
                        );
                    }
                };

            match run_command(&state.sandbox, &params, state.session.allow_command).await {
                Ok(res) => RpcResponse::success(req_id, serde_json::to_value(res).unwrap()),
                Err(crate::command::CommandError::NotAllowed) => RpcResponse::error(
                    req_id,
                    error_codes::COMMAND_NOT_ALLOWED,
                    "Command execution is disabled. Enable --allow-command in daemon flags or Settings.",
                    None,
                ),
                Err(e) => RpcResponse::error(req_id, error_codes::INTERNAL_ERROR, e.to_string(), None),
            }
        }

        "workspace.list_dir" => {
            let params: ListDirParams = match req.params {
                Some(p) => match serde_json::from_value(p) {
                    Ok(params) => params,
                    Err(_) => {
                        return RpcResponse::error(
                            req_id,
                            error_codes::INTERNAL_ERROR,
                            "workspace.list_dir requires valid parameters",
                            None,
                        );
                    }
                },
                None => ListDirParams {
                    path: None,
                    max_entries: None,
                },
            };

            let req_path = params.path.as_deref().unwrap_or(".");
            let trimmed = req_path.trim();
            let lookup_path = if trimmed.is_empty() { "." } else { trimmed };

            let resolved = match state.sandbox.resolve(lookup_path) {
                Ok(p) => p,
                Err(e) => return map_sandbox_error(req_id, e),
            };

            if !resolved.exists() {
                return RpcResponse::error(
                    req_id,
                    error_codes::NOT_FOUND,
                    format!("Directory not found: {lookup_path}"),
                    None,
                );
            }

            if !resolved.is_dir() {
                return RpcResponse::error(
                    req_id,
                    error_codes::INTERNAL_ERROR,
                    format!("Path is not a directory: {lookup_path}"),
                    None,
                );
            }

            let read_dir = match std::fs::read_dir(&resolved) {
                Ok(rd) => rd,
                Err(e) => {
                    return RpcResponse::error(
                        req_id,
                        error_codes::INTERNAL_ERROR,
                        format!("Failed to read directory: {e}"),
                        None,
                    );
                }
            };

            let mut entries = Vec::new();
            for entry in read_dir {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                let file_name = entry.file_name().to_string_lossy().to_string();
                let meta = entry.metadata().ok();
                let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                let size = if is_dir {
                    0
                } else {
                    meta.as_ref().map(|m| m.len()).unwrap_or(0)
                };
                let modified = meta
                    .and_then(|m| m.modified().ok())
                    .map(|st| chrono::DateTime::<chrono::Utc>::from(st).to_rfc3339());

                entries.push(DirEntryInfo {
                    name: file_name,
                    is_dir,
                    size,
                    modified,
                });
            }

            entries.sort_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                    .then_with(|| a.name.cmp(&b.name))
            });

            let total = entries.len();
            let max_entries = params.max_entries.unwrap_or(200);
            let truncated = total > max_entries;
            if truncated {
                entries.truncate(max_entries);
            }

            let rel_path = match state.sandbox.to_relative(&resolved) {
                Ok(rel) if !rel.is_empty() => rel,
                _ => ".".to_string(),
            };

            let result = ListDirResult {
                path: rel_path,
                entries,
                total,
                truncated,
            };

            RpcResponse::success(req_id, serde_json::to_value(result).unwrap())
        }

        "workspace.glob" => {
            let params: GlobParams = match req.params {
                Some(p) => match serde_json::from_value(p) {
                    Ok(params) => params,
                    Err(_) => {
                        return RpcResponse::error(
                            req_id,
                            error_codes::INTERNAL_ERROR,
                            "workspace.glob requires valid parameters",
                            None,
                        );
                    }
                },
                None => {
                    return RpcResponse::error(
                        req_id,
                        error_codes::INTERNAL_ERROR,
                        "workspace.glob requires parameters",
                        None,
                    );
                }
            };

            let req_path = params.path.as_deref().unwrap_or(".");
            let trimmed = req_path.trim();
            let lookup_path = if trimmed.is_empty() { "." } else { trimmed };

            let resolved_base = match state.sandbox.resolve(lookup_path) {
                Ok(p) => p,
                Err(e) => return map_sandbox_error(req_id, e),
            };

            if !resolved_base.exists() {
                return RpcResponse::error(
                    req_id,
                    error_codes::NOT_FOUND,
                    format!("Path not found: {lookup_path}"),
                    None,
                );
            }

            if !resolved_base.is_dir() {
                return RpcResponse::error(
                    req_id,
                    error_codes::INTERNAL_ERROR,
                    format!("Path is not a directory: {lookup_path}"),
                    None,
                );
            }

            let matcher = match globset::GlobBuilder::new(&params.pattern)
                .case_insensitive(true)
                .literal_separator(false)
                .build()
            {
                Ok(g) => g.compile_matcher(),
                Err(e) => {
                    return RpcResponse::error(
                        req_id,
                        error_codes::INTERNAL_ERROR,
                        format!("Invalid glob pattern: {e}"),
                        None,
                    );
                }
            };

            let mut builder = ignore::WalkBuilder::new(&resolved_base);
            builder
                .standard_filters(true)
                .require_git(false)
                .hidden(true);

            let walker = builder.build();
            let mut matches = Vec::new();

            for entry in walker {
                let dir_entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                let path = dir_entry.path();
                if path == resolved_base {
                    continue;
                }

                let rel_ws = match state.sandbox.to_relative(path) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                let rel_base = match path.strip_prefix(&resolved_base) {
                    Ok(p) => p.to_string_lossy().replace('\\', "/"),
                    Err(_) => continue,
                };

                if matcher.is_match(&rel_base) || matcher.is_match(&rel_ws) {
                    matches.push(rel_ws);
                }
            }

            matches.sort();
            matches.dedup();

            let total = matches.len();
            let max_matches = params.max_matches.unwrap_or(200);
            let truncated = total > max_matches;
            if truncated {
                matches.truncate(max_matches);
            }

            let result = GlobResult {
                matches,
                total,
                truncated,
            };

            RpcResponse::success(req_id, serde_json::to_value(result).unwrap())
        }

        _ => RpcResponse::error(
            req_id,
            error_codes::INTERNAL_ERROR,
            format!("Unknown workspace method: {}", req.method),
            None,
        ),
    }
}

fn map_sandbox_error(req_id: String, err: SandboxError) -> RpcResponse {
    match err {
        SandboxError::PathOutsideWorkspace(p) => RpcResponse::error(
            req_id,
            error_codes::PATH_OUTSIDE_WORKSPACE,
            format!("Path outside authorized workspace: {p}"),
            None,
        ),
        SandboxError::TraversalNotAllowed(p) => RpcResponse::error(
            req_id,
            error_codes::PATH_OUTSIDE_WORKSPACE,
            format!("Directory traversal is prohibited: {p}"),
            None,
        ),
        SandboxError::ReservedDeviceName(d) => RpcResponse::error(
            req_id,
            error_codes::PATH_OUTSIDE_WORKSPACE,
            format!("Reserved device name not allowed: {d}"),
            None,
        ),
        SandboxError::RootNotFound(p) => RpcResponse::error(
            req_id,
            error_codes::WORKSPACE_NOT_AUTHORIZED,
            format!("Workspace root not found: {p}"),
            None,
        ),
        _ => RpcResponse::error(req_id, error_codes::INTERNAL_ERROR, err.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_server_status_and_auth() {
        let temp = tempdir().unwrap();
        let sandbox = Arc::new(PathSandbox::new(temp.path()).unwrap());
        let auth = Arc::new(AuthManager::new(Some("test_tok".to_string()), None));
        let session = Arc::new(WorkspaceSession::new(
            temp.path().to_path_buf(),
            true,
            false,
        ));
        let (event_tx, _) = broadcast::channel(16);

        let state = ServerState {
            sandbox,
            auth,
            session,
            event_tx,
        };

        // Test unauthenticated status request
        let status_req = RpcRequest {
            v: 1,
            id: "1".to_string(),
            method: "workspace.status".to_string(),
            params: None,
        };
        let resp = dispatch_method(&state, status_req).await;
        assert!(resp.ok);
        let val: WorkspaceStatusResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(val.connected);
        assert!(val.read);
        assert!(val.write);
        assert!(!val.command);
        assert!(val.guidance.is_some());
        assert!(val.guidance.unwrap().contains("The workspace root is"));
    }
}
