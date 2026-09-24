use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::SystemTime;
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use webbrain_workspace::auth::AuthManager;
use webbrain_workspace::files::{atomic_write, read_text_file, LineEnding};
use webbrain_workspace::patch::{apply_patch, PatchError};
use webbrain_workspace::paths::PathSandbox;
use webbrain_workspace::protocol::{
    error_codes, ApplyPatchParams, ApplyPatchResult, AuthHandshakeParams, CreateFileParams,
    CreateFileResult, RpcRequest, RpcResponse, PROTOCOL_VERSION,
};
use webbrain_workspace::server::{run_server, ServerState};
use webbrain_workspace::session::WorkspaceSession;

#[test]
fn test_exact_patch_success() {
    let original = "function calculateTotal(items) {\n    let total = 0;\n    return total;\n}\n";
    let res = apply_patch(
        original,
        Some("let total = 0;"),
        Some("let total = 0;\n    for (const item of items) total += item.price;"),
        None,
        "calc.js",
    )
    .unwrap();

    assert!(res.new_content.contains("for (const item of items)"));
    assert_eq!(res.lines_removed, 1);
    assert_eq!(res.lines_added, 2);
    assert!(res.summary.contains("+2 / -1"));
}

#[test]
fn test_exact_patch_ambiguous_context_rejection() {
    let original = "console.log('hi');\nconsole.log('hi');\n";
    let err = apply_patch(
        original,
        Some("console.log('hi');"),
        Some("console.log('bye');"),
        None,
        "test.js",
    )
    .unwrap_err();

    match err {
        PatchError::AmbiguousContext { occurrences, .. } => assert_eq!(occurrences, 2),
        _ => panic!("Expected AmbiguousContext, got {err:?}"),
    }
}

#[test]
fn test_exact_patch_context_not_found() {
    let original = "const a = 1;\n";
    let err = apply_patch(
        original,
        Some("const missing = 2;"),
        Some("const a = 2;"),
        None,
        "test.js",
    )
    .unwrap_err();

    assert!(matches!(err, PatchError::ContextNotFound(_)));
}

#[test]
fn test_unified_diff_patch() {
    let original = "line1\nline2\nline3\nline4\n";
    let patch = "@@ -2,2 +2,2 @@\n-line2\n+line2_modified\n line3\n";
    let res = apply_patch(original, None, None, Some(patch), "test.txt").unwrap();
    assert_eq!(res.new_content, "line1\nline2_modified\nline3\nline4\n");
    assert_eq!(res.lines_added, 1);
    assert_eq!(res.lines_removed, 1);
}

#[test]
fn test_session_revision_tracking_and_conflict() {
    let temp = tempdir().unwrap();
    let session = WorkspaceSession::new(temp.path().to_path_buf(), true, false);
    let mtime = SystemTime::now();

    // 1. Initial read assigns revision 1
    let rev1 = session.record_read("src/app.js", "hash_v1", mtime, 500);
    assert_eq!(rev1, 1);

    // 2. Successful patch with expectedRevision = 1 updates to revision 2
    session.check_revision("src/app.js", 1, Some("hash_v1"), "hash_v1").unwrap();
    let (old_rev, new_rev) = session.commit_revision("src/app.js", "hash_v2", mtime, 520);
    assert_eq!(old_rev, 1);
    assert_eq!(new_rev, 2);

    // 3. Stale edit with expectedRevision = 1 is rejected
    let err = session
        .check_revision("src/app.js", 1, Some("hash_v1"), "hash_v2")
        .unwrap_err();

    match err {
        PatchError::RevisionConflict {
            expected_revision,
            current_revision,
            ..
        } => {
            assert_eq!(expected_revision, 1);
            assert_eq!(current_revision, 2);
        }
        _ => panic!("Expected RevisionConflict, got {err:?}"),
    }
}

#[test]
fn test_patch_atomic_write_preserves_crlf_and_bom() {
    let temp = tempdir().unwrap();
    let file_path = temp.path().join("crlf_bom.txt");

    // Write initial CRLF + BOM file
    atomic_write(
        &file_path,
        "Alpha\r\nBeta\r\n",
        true,
        Some(LineEnding::CrLf),
    )
    .unwrap();

    let initial = read_text_file(&file_path, None).unwrap();
    assert!(initial.has_bom);
    assert_eq!(initial.line_ending, LineEnding::CrLf);

    // Apply patch
    let patched = apply_patch(
        &initial.content,
        Some("Beta"),
        Some("BetaPatched"),
        None,
        "crlf_bom.txt",
    )
    .unwrap();

    // Write back atomically preserving BOM and CRLF
    let new_hash = atomic_write(
        &file_path,
        &patched.new_content,
        initial.has_bom,
        Some(initial.line_ending),
    )
    .unwrap();

    let updated = read_text_file(&file_path, None).unwrap();
    assert!(updated.has_bom);
    assert_eq!(updated.line_ending, LineEnding::CrLf);
    assert_eq!(updated.content, "Alpha\r\nBetaPatched\r\n");
    assert_eq!(updated.hash, new_hash);
}

struct TestServer {
    pub port: u16,
    pub token: String,
    pub sandbox_path: std::path::PathBuf,
    _shutdown_tx: watch::Sender<bool>,
}

async fn start_test_server() -> TestServer {
    let temp = tempdir().unwrap();
    let sandbox_path = temp.path().to_path_buf();
    std::mem::forget(temp);

    let sandbox = Arc::new(PathSandbox::new(&sandbox_path).unwrap());
    let token = "test_token_patch".to_string();
    let auth = Arc::new(AuthManager::new(Some(token.clone()), None));
    let session = Arc::new(WorkspaceSession::new(sandbox_path.clone(), true, true));
    let (event_tx, _) = broadcast::channel(64);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let state = ServerState {
        sandbox,
        auth,
        session,
        event_tx,
    };

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tokio::spawn(async move {
        let _ = run_server(listener, state, shutdown_rx).await;
    });

    TestServer {
        port,
        token,
        sandbox_path,
        _shutdown_tx: shutdown_tx,
    }
}

#[tokio::test]
async fn test_apply_patch_create_new_file() {
    let server = start_test_server().await;
    let url = format!("ws://127.0.0.1:{}", server.port);
    let (mut ws_stream, _) = connect_async(&url).await.unwrap();

    // Authenticate
    let auth_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "auth".to_string(),
        method: "auth.handshake".to_string(),
        params: Some(
            serde_json::to_value(AuthHandshakeParams {
                token: server.token.clone(),
                client: None,
                extension_id: None,
                protocol_version: Some(PROTOCOL_VERSION),
            })
            .unwrap(),
        ),
    };
    ws_stream
        .send(Message::Text(serde_json::to_string(&auth_req).unwrap().into()))
        .await
        .unwrap();
    let _auth_resp = ws_stream.next().await.unwrap().unwrap();

    // Create new file via workspace.apply_patch with expectedRevision: 0, oldText: "", newText: "hello"
    let patch_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "req_create_via_patch".to_string(),
        method: "workspace.apply_patch".to_string(),
        params: Some(
            serde_json::to_value(ApplyPatchParams {
                path: "selam.md".to_string(),
                expected_revision: 0,
                expected_hash: None,
                old_text: Some("".to_string()),
                new_text: Some("hello".to_string()),
                patch: None,
            })
            .unwrap(),
        ),
    };
    ws_stream
        .send(Message::Text(serde_json::to_string(&patch_req).unwrap().into()))
        .await
        .unwrap();

    use futures_util::StreamExt;
    let resp_msg = ws_stream.next().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&resp_msg.to_string()).unwrap();
    assert!(resp.ok, "Expected ok response, got error: {:?}", resp.error);
    let result: ApplyPatchResult = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert_eq!(result.old_revision, 0);
    assert_eq!(result.new_revision, 1);

    // Verify file actually exists and contains "hello"
    let created_path = server.sandbox_path.join("selam.md");
    assert!(created_path.exists());
    let read_res = read_text_file(&created_path, None).unwrap();
    assert_eq!(read_res.content, "hello");
    assert_eq!(read_res.hash, result.new_hash);
    // Creating again on non-existent file with expected_revision > 1 should fail
    let bad_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "req_bad".to_string(),
        method: "workspace.apply_patch".to_string(),
        params: Some(
            serde_json::to_value(ApplyPatchParams {
                path: "nonexistent.md".to_string(),
                expected_revision: 2,
                expected_hash: None,
                old_text: Some("".to_string()),
                new_text: Some("should fail".to_string()),
                patch: None,
            })
            .unwrap(),
        ),
    };
    ws_stream
        .send(Message::Text(serde_json::to_string(&bad_req).unwrap().into()))
        .await
        .unwrap();
    let bad_msg = ws_stream.next().await.unwrap().unwrap();
    let bad_resp: RpcResponse = serde_json::from_str(&bad_msg.to_string()).unwrap();
    assert!(!bad_resp.ok);
    assert_eq!(bad_resp.error.unwrap().code, error_codes::REVISION_CONFLICT);
}

#[tokio::test]
async fn test_workspace_create_file() {
    let server = start_test_server().await;
    let url = format!("ws://127.0.0.1:{}", server.port);
    let (mut ws_stream, _) = connect_async(&url).await.unwrap();

    // Authenticate
    let auth_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "auth".to_string(),
        method: "auth.handshake".to_string(),
        params: Some(
            serde_json::to_value(AuthHandshakeParams {
                token: server.token.clone(),
                client: None,
                extension_id: None,
                protocol_version: Some(PROTOCOL_VERSION),
            })
            .unwrap(),
        ),
    };
    ws_stream
        .send(Message::Text(serde_json::to_string(&auth_req).unwrap().into()))
        .await
        .unwrap();
    let _auth_resp = ws_stream.next().await.unwrap().unwrap();

    // 1. Create a new file
    let create_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "req_create".to_string(),
        method: "workspace.create_file".to_string(),
        params: Some(
            serde_json::to_value(CreateFileParams {
                path: "test_new.txt".to_string(),
                content: "initial file content\n".to_string(),
                overwrite: None,
            })
            .unwrap(),
        ),
    };
    ws_stream
        .send(Message::Text(serde_json::to_string(&create_req).unwrap().into()))
        .await
        .unwrap();
    let create_msg = ws_stream.next().await.unwrap().unwrap();
    let create_resp: RpcResponse = serde_json::from_str(&create_msg.to_string()).unwrap();
    assert!(create_resp.ok, "Expected create to succeed: {:?}", create_resp.error);
    let res: CreateFileResult = serde_json::from_value(create_resp.result.unwrap()).unwrap();
    assert_eq!(res.revision, 1);
    assert_eq!(res.path, "test_new.txt");

    // 2. Try creating the same file without overwrite -> must fail
    let dup_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "req_dup".to_string(),
        method: "workspace.create_file".to_string(),
        params: Some(
            serde_json::to_value(CreateFileParams {
                path: "test_new.txt".to_string(),
                content: "different content".to_string(),
                overwrite: Some(false),
            })
            .unwrap(),
        ),
    };
    ws_stream
        .send(Message::Text(serde_json::to_string(&dup_req).unwrap().into()))
        .await
        .unwrap();
    let dup_msg = ws_stream.next().await.unwrap().unwrap();
    let dup_resp: RpcResponse = serde_json::from_str(&dup_msg.to_string()).unwrap();
    assert!(!dup_resp.ok);
    assert_eq!(dup_resp.error.unwrap().code, error_codes::INTERNAL_ERROR);

    // 3. Overwrite the file with overwrite = true
    let overwrite_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "req_overwrite".to_string(),
        method: "workspace.create_file".to_string(),
        params: Some(
            serde_json::to_value(CreateFileParams {
                path: "test_new.txt".to_string(),
                content: "overwritten content".to_string(),
                overwrite: Some(true),
            })
            .unwrap(),
        ),
    };
    ws_stream
        .send(Message::Text(serde_json::to_string(&overwrite_req).unwrap().into()))
        .await
        .unwrap();
    let overwrite_msg = ws_stream.next().await.unwrap().unwrap();
    let overwrite_resp: RpcResponse = serde_json::from_str(&overwrite_msg.to_string()).unwrap();
    assert!(overwrite_resp.ok);
    let over_res: CreateFileResult = serde_json::from_value(overwrite_resp.result.unwrap()).unwrap();
    assert_eq!(over_res.revision, 2);

    let target = server.sandbox_path.join("test_new.txt");
    let read_res = read_text_file(&target, None).unwrap();
    assert_eq!(read_res.content, "overwritten content");
}
