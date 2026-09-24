use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use webbrain_workspace::auth::AuthManager;
use webbrain_workspace::paths::PathSandbox;
use webbrain_workspace::protocol::{
    error_codes, ApplyPatchParams, ApplyPatchResult, AuthHandshakeParams, AuthHandshakeResult,
    GlobParams, GlobResult, ListDirParams, ListDirResult, ReadFileParams, ReadFileResult,
    RpcRequest, RpcResponse, WorkspaceStatusResult, PROTOCOL_VERSION,
};
use webbrain_workspace::server::{run_server, ServerState};
use webbrain_workspace::session::WorkspaceSession;

struct TestServer {
    pub port: u16,
    pub token: String,
    pub sandbox_path: std::path::PathBuf,
    _shutdown_tx: watch::Sender<bool>,
}

async fn start_test_server() -> TestServer {
    let temp = tempdir().unwrap();
    let sandbox_path = temp.path().to_path_buf();
    // Keep tempdir from dropping prematurely by leaking its path for test duration
    std::mem::forget(temp);

    let sandbox = Arc::new(PathSandbox::new(&sandbox_path).unwrap());
    let token = "test_secret_token_123".to_string();
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
async fn test_protocol_e2e_lifecycle() {
    let server = start_test_server().await;
    let url = format!("ws://127.0.0.1:{}", server.port);

    // 1. Connect without origin header (native client)
    let (mut ws_stream, _) = connect_async(&url).await.unwrap();

    // 2. Attempt command before handshake -> must return UNAUTHENTICATED
    let status_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "req_1".to_string(),
        method: "workspace.status".to_string(),
        params: None,
    };
    ws_stream
        .send(Message::Text(
            serde_json::to_string(&status_req).unwrap().into(),
        ))
        .await
        .unwrap();

    let msg = ws_stream.next().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&msg.to_string()).unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.error.unwrap().code, error_codes::UNAUTHENTICATED);

    // 3. Handshake with invalid token -> rejected and closed
    let bad_handshake = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "req_bad_auth".to_string(),
        method: "auth.handshake".to_string(),
        params: Some(
            serde_json::to_value(AuthHandshakeParams {
                token: "wrong_token".to_string(),
                client: Some("test-client".to_string()),
                extension_id: None,
                protocol_version: Some(1),
            })
            .unwrap(),
        ),
    };
    ws_stream
        .send(Message::Text(
            serde_json::to_string(&bad_handshake).unwrap().into(),
        ))
        .await
        .unwrap();

    let auth_msg = ws_stream.next().await.unwrap().unwrap();
    let auth_resp: RpcResponse = serde_json::from_str(&auth_msg.to_string()).unwrap();
    assert!(!auth_resp.ok);
    assert_eq!(auth_resp.error.unwrap().code, error_codes::UNAUTHENTICATED);

    // 4. Reconnect and handshake with valid token
    let (mut ws_stream2, _) = connect_async(&url).await.unwrap();
    let good_handshake = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "req_auth_ok".to_string(),
        method: "auth.handshake".to_string(),
        params: Some(
            serde_json::to_value(AuthHandshakeParams {
                token: server.token.clone(),
                client: Some("webbrain-extension".to_string()),
                extension_id: Some("abcdefg".to_string()),
                protocol_version: Some(1),
            })
            .unwrap(),
        ),
    };
    ws_stream2
        .send(Message::Text(
            serde_json::to_string(&good_handshake).unwrap().into(),
        ))
        .await
        .unwrap();

    let ok_msg = ws_stream2.next().await.unwrap().unwrap();
    let ok_resp: RpcResponse = serde_json::from_str(&ok_msg.to_string()).unwrap();
    assert!(ok_resp.ok);
    let auth_res: AuthHandshakeResult = serde_json::from_value(ok_resp.result.unwrap()).unwrap();
    assert_eq!(auth_res.protocol_version, PROTOCOL_VERSION);
    assert!(auth_res.capabilities.contains(&"write".to_string()));
    assert!(auth_res.capabilities.contains(&"command".to_string()));

    // 5. Test workspace.status
    let status_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "req_status".to_string(),
        method: "workspace.status".to_string(),
        params: None,
    };
    ws_stream2
        .send(Message::Text(
            serde_json::to_string(&status_req).unwrap().into(),
        ))
        .await
        .unwrap();

    let stat_msg = ws_stream2.next().await.unwrap().unwrap();
    let stat_resp: RpcResponse = serde_json::from_str(&stat_msg.to_string()).unwrap();
    assert!(stat_resp.ok);
    let stat_res: WorkspaceStatusResult =
        serde_json::from_value(stat_resp.result.unwrap()).unwrap();
    assert!(stat_res.connected);
    assert!(stat_res.write);
    assert!(stat_res.command);

    // 6. Create a test file and test workspace.read_file & workspace.read_range
    let test_file = server.sandbox_path.join("example.js");
    std::fs::write(
        &test_file,
        "console.log('line 1');\nconsole.log('line 2');\nconsole.log('line 3');\n",
    )
    .unwrap();

    let read_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "req_read".to_string(),
        method: "workspace.read_file".to_string(),
        params: Some(
            serde_json::to_value(ReadFileParams {
                path: "example.js".to_string(),
                max_chars: None,
            })
            .unwrap(),
        ),
    };
    ws_stream2
        .send(Message::Text(
            serde_json::to_string(&read_req).unwrap().into(),
        ))
        .await
        .unwrap();

    let read_msg = ws_stream2.next().await.unwrap().unwrap();
    let read_resp: RpcResponse = serde_json::from_str(&read_msg.to_string()).unwrap();
    assert!(read_resp.ok);
    let read_res: ReadFileResult = serde_json::from_value(read_resp.result.unwrap()).unwrap();
    assert_eq!(read_res.revision, 1);
    assert!(read_res.content.contains("line 1"));

    // 7. Test workspace.apply_patch
    let patch_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "req_patch".to_string(),
        method: "workspace.apply_patch".to_string(),
        params: Some(
            serde_json::to_value(ApplyPatchParams {
                path: "example.js".to_string(),
                expected_revision: 1,
                expected_hash: Some(read_res.hash),
                old_text: Some("line 2".to_string()),
                new_text: Some("line 2 modified".to_string()),
                patch: None,
            })
            .unwrap(),
        ),
    };
    ws_stream2
        .send(Message::Text(
            serde_json::to_string(&patch_req).unwrap().into(),
        ))
        .await
        .unwrap();

    let patch_msg = ws_stream2.next().await.unwrap().unwrap();
    let patch_resp: RpcResponse = serde_json::from_str(&patch_msg.to_string()).unwrap();
    assert!(patch_resp.ok);
    let patch_res: ApplyPatchResult = serde_json::from_value(patch_resp.result.unwrap()).unwrap();
    assert_eq!(patch_res.old_revision, 1);
    assert_eq!(patch_res.new_revision, 2);

    // 8. Stale patch with expected_revision = 1 must now fail with REVISION_CONFLICT
    let stale_patch_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "req_stale".to_string(),
        method: "workspace.apply_patch".to_string(),
        params: Some(
            serde_json::to_value(ApplyPatchParams {
                path: "example.js".to_string(),
                expected_revision: 1, // Stale!
                expected_hash: None,
                old_text: Some("line 1".to_string()),
                new_text: Some("line 1 updated".to_string()),
                patch: None,
            })
            .unwrap(),
        ),
    };
    ws_stream2
        .send(Message::Text(
            serde_json::to_string(&stale_patch_req).unwrap().into(),
        ))
        .await
        .unwrap();

    let stale_msg = ws_stream2.next().await.unwrap().unwrap();
    let stale_resp: RpcResponse = serde_json::from_str(&stale_msg.to_string()).unwrap();
    assert!(!stale_resp.ok);
    assert_eq!(
        stale_resp.error.unwrap().code,
        error_codes::REVISION_CONFLICT
    );
}

#[tokio::test]
async fn test_protocol_origin_rejection() {
    let server = start_test_server().await;
    let url = format!("ws://127.0.0.1:{}", server.port);

    // Connect with a Web Origin header (e.g. from a malicious webpage)
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Origin", "https://attacker.example.com".parse().unwrap());

    let (mut ws_stream, _) = connect_async(request).await.unwrap();

    // The server must reject and close the socket immediately
    let next_msg = ws_stream.next().await;
    match next_msg {
        Some(Ok(Message::Close(Some(frame)))) => {
            assert!(frame.reason.contains("Untrusted Origin") || frame.reason.contains("Origin"));
        }
        Some(Ok(Message::Close(None))) | None => {
            // Socket closed cleanly by server
        }
        other => panic!("Expected connection closure for untrusted origin, got {other:?}"),
    }
}

#[tokio::test]
async fn test_workspace_list_dir() {
    let server = start_test_server().await;
    let url = format!("ws://127.0.0.1:{}", server.port);

    // Create directories and files
    std::fs::create_dir_all(server.sandbox_path.join("dir_b")).unwrap();
    std::fs::create_dir_all(server.sandbox_path.join("dir_a")).unwrap();
    std::fs::write(server.sandbox_path.join("file_z.txt"), "hello z").unwrap();
    std::fs::write(server.sandbox_path.join("file_a.txt"), "hello a").unwrap();
    std::fs::write(server.sandbox_path.join("dir_a").join("nested.txt"), "inside dir_a").unwrap();

    let (mut ws, _) = connect_async(&url).await.unwrap();

    // Handshake
    let handshake_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "auth_1".to_string(),
        method: "auth.handshake".to_string(),
        params: Some(
            serde_json::to_value(AuthHandshakeParams {
                token: server.token.clone(),
                client: Some("test-agent".to_string()),
                extension_id: None,
                protocol_version: Some(PROTOCOL_VERSION),
            })
            .unwrap(),
        ),
    };
    ws.send(Message::Text(serde_json::to_string(&handshake_req).unwrap().into()))
        .await
        .unwrap();
    let _auth_resp = ws.next().await.unwrap().unwrap();

    // 1. List root directory
    let list_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "list_1".to_string(),
        method: "workspace.list_dir".to_string(),
        params: Some(serde_json::to_value(ListDirParams {
            path: None,
            max_entries: None,
        }).unwrap()),
    };
    ws.send(Message::Text(serde_json::to_string(&list_req).unwrap().into()))
        .await
        .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&msg.to_string()).unwrap();
    assert!(resp.ok, "Expected ok response, got: {:?}", resp.error);
    let result: ListDirResult = serde_json::from_value(resp.result.unwrap()).unwrap();

    assert_eq!(result.total, 4);
    assert!(!result.truncated);
    // Directories first, sorted alphabetically: dir_a, dir_b, file_a.txt, file_z.txt
    assert_eq!(result.entries[0].name, "dir_a");
    assert!(result.entries[0].is_dir);
    assert_eq!(result.entries[0].size, 0);
    assert!(result.entries[0].modified.is_some());

    assert_eq!(result.entries[1].name, "dir_b");
    assert!(result.entries[1].is_dir);

    assert_eq!(result.entries[2].name, "file_a.txt");
    assert!(!result.entries[2].is_dir);
    assert_eq!(result.entries[2].size, 7);
    assert!(result.entries[2].modified.is_some());

    assert_eq!(result.entries[3].name, "file_z.txt");
    assert!(!result.entries[3].is_dir);

    // 2. List with max_entries truncation
    let list_trunc_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "list_2".to_string(),
        method: "workspace.list_dir".to_string(),
        params: Some(serde_json::to_value(ListDirParams {
            path: Some(".".to_string()),
            max_entries: Some(2),
        }).unwrap()),
    };
    ws.send(Message::Text(serde_json::to_string(&list_trunc_req).unwrap().into()))
        .await
        .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&msg.to_string()).unwrap();
    let result: ListDirResult = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert_eq!(result.total, 4);
    assert!(result.truncated);
    assert_eq!(result.entries.len(), 2);

    // 3. List subdirectory dir_a
    let list_sub_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "list_3".to_string(),
        method: "workspace.list_dir".to_string(),
        params: Some(serde_json::to_value(ListDirParams {
            path: Some("dir_a".to_string()),
            max_entries: None,
        }).unwrap()),
    };
    ws.send(Message::Text(serde_json::to_string(&list_sub_req).unwrap().into()))
        .await
        .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&msg.to_string()).unwrap();
    let result: ListDirResult = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.entries[0].name, "nested.txt");

    // 4. List a file path -> error
    let list_file_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "list_4".to_string(),
        method: "workspace.list_dir".to_string(),
        params: Some(serde_json::to_value(ListDirParams {
            path: Some("file_a.txt".to_string()),
            max_entries: None,
        }).unwrap()),
    };
    ws.send(Message::Text(serde_json::to_string(&list_file_req).unwrap().into()))
        .await
        .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&msg.to_string()).unwrap();
    assert!(!resp.ok);
    assert!(resp.error.unwrap().message.contains("not a directory"));
}

#[tokio::test]
async fn test_workspace_glob() {
    let server = start_test_server().await;
    let url = format!("ws://127.0.0.1:{}", server.port);

    // Create directory layout
    let src = server.sandbox_path.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("main.rs"), "fn main() {}").unwrap();
    std::fs::write(src.join("lib.rs"), "pub fn lib() {}").unwrap();

    let tests_dir = server.sandbox_path.join("tests");
    std::fs::create_dir_all(&tests_dir).unwrap();
    std::fs::write(tests_dir.join("test.rs"), "// test").unwrap();

    std::fs::write(server.sandbox_path.join("README.md"), "# Readme").unwrap();

    // Gitignore file ignoring ignored.rs
    std::fs::write(server.sandbox_path.join(".gitignore"), "ignored.rs\n").unwrap();
    std::fs::write(server.sandbox_path.join("ignored.rs"), "// ignored").unwrap();

    let (mut ws, _) = connect_async(&url).await.unwrap();

    // Handshake
    let handshake_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "auth_1".to_string(),
        method: "auth.handshake".to_string(),
        params: Some(
            serde_json::to_value(AuthHandshakeParams {
                token: server.token.clone(),
                client: Some("test-agent".to_string()),
                extension_id: None,
                protocol_version: Some(PROTOCOL_VERSION),
            })
            .unwrap(),
        ),
    };
    ws.send(Message::Text(serde_json::to_string(&handshake_req).unwrap().into()))
        .await
        .unwrap();
    let _auth_resp = ws.next().await.unwrap().unwrap();

    // 1. Glob all .rs files (respecting .gitignore)
    let glob_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "glob_1".to_string(),
        method: "workspace.glob".to_string(),
        params: Some(serde_json::to_value(GlobParams {
            pattern: "**/*.rs".to_string(),
            path: None,
            max_matches: None,
        }).unwrap()),
    };
    ws.send(Message::Text(serde_json::to_string(&glob_req).unwrap().into()))
        .await
        .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&msg.to_string()).unwrap();
    assert!(resp.ok, "Expected ok, got: {:?}", resp.error);
    let result: GlobResult = serde_json::from_value(resp.result.unwrap()).unwrap();

    assert_eq!(result.total, 3);
    assert!(!result.truncated);
    assert!(result.matches.contains(&"src/lib.rs".to_string()));
    assert!(result.matches.contains(&"src/main.rs".to_string()));
    assert!(result.matches.contains(&"tests/test.rs".to_string()));
    // Verify ignored.rs is omitted
    assert!(!result.matches.contains(&"ignored.rs".to_string()));
    // Verify README.md is omitted
    assert!(!result.matches.contains(&"README.md".to_string()));

    // 2. Glob scoped to src subdirectory
    let glob_sub_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "glob_2".to_string(),
        method: "workspace.glob".to_string(),
        params: Some(serde_json::to_value(GlobParams {
            pattern: "*.rs".to_string(),
            path: Some("src".to_string()),
            max_matches: None,
        }).unwrap()),
    };
    ws.send(Message::Text(serde_json::to_string(&glob_sub_req).unwrap().into()))
        .await
        .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&msg.to_string()).unwrap();
    let result: GlobResult = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert_eq!(result.total, 2);
    assert!(result.matches.contains(&"src/lib.rs".to_string()));
    assert!(result.matches.contains(&"src/main.rs".to_string()));

    // 3. Glob with max_matches truncation
    let glob_trunc_req = RpcRequest {
        v: PROTOCOL_VERSION,
        id: "glob_3".to_string(),
        method: "workspace.glob".to_string(),
        params: Some(serde_json::to_value(GlobParams {
            pattern: "**/*.rs".to_string(),
            path: None,
            max_matches: Some(1),
        }).unwrap()),
    };
    ws.send(Message::Text(serde_json::to_string(&glob_trunc_req).unwrap().into()))
        .await
        .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&msg.to_string()).unwrap();
    let result: GlobResult = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert_eq!(result.total, 3);
    assert!(result.truncated);
    assert_eq!(result.matches.len(), 1);
}
