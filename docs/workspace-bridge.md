# WebBrain Local Workspace Bridge

The **Local Workspace Bridge** extends WebBrain so that its in-browser autonomous AI agent can inspect, search, read, patch, diff, and validate code in a user-authorized local project directory through a high-speed, persistent loopback connection to a local Rust daemon (`workspace-bridge`).

Rather than turning WebBrain into an IDE or terminal emulator, the Workspace Bridge provides a tight, low-latency, safety-hardened coding loop that complements existing browser automation, MCP tools, and WebBrain capabilities.

---

## 1. Architecture & End-to-End Data Flow

```text
┌────────────────────────────────────────────────────────┐
│               WebBrain Sidepanel UI                    │
│   (Workspace status badge, connect/pair settings)      │
└───────────────────────────┬────────────────────────────┘
                            │ chrome.runtime.sendMessage
                            ▼
┌────────────────────────────────────────────────────────┐
│            Background Service Worker                   │
│                                                        │
│  background.js  ──► workspace-runs.js (session state)  │
│                           │                            │
│  agent.js (Agent loop)    │                            │
│   ├─ tools.js             │                            │
│   ├─ permission-gate.js   │                            │
│   └─ workspace-client.js ◄┘                            │
└───────────────────────────┬────────────────────────────┘
                            │ internal message (workspace_bridge_call)
                            ▼
┌────────────────────────────────────────────────────────┐
│        Chrome Offscreen Document (offscreen.html)      │
│                                                        │
│  existing: recorder.js, cloud-bridge.js                │
│  workspace-bridge.js                                   │
│    • Outbound WebSocket to 127.0.0.1:18374             │
│    • Request correlation (Map<id, Promise>)            │
│    • Event forwarder (file.changed to background)      │
└───────────────────────────┬────────────────────────────┘
                            │ persistent localhost WebSocket (ws://127.0.0.1:18374)
                            │ authenticated via pairing token
                            ▼
┌────────────────────────────────────────────────────────┐
│        Rust Workspace Daemon (webbrain-workspace)      │
│                                                        │
│  server.rs    ──► Tokio WebSocket server               │
│  auth.rs      ──► Token pairing & origin verification  │
│  session.rs   ──► Session, root boundary, revisions    │
│  paths.rs     ──► Windows path canonicalization & jail │
│  search.rs    ──► Codebase search (ignore-aware)       │
│  patch.rs     ──► Context-aware atomic patch engine    │
│  watcher.rs   ──► Notify-based filesystem watcher      │
│  git.rs       ──► Native git diff runner               │
│  command.rs   ──► Gated child process runner           │
└───────────────────────────┬────────────────────────────┘
                            │ file I/O & git
                            ▼
┌────────────────────────────────────────────────────────┐
│            Authorized Local Project Root               │
│               (e.g., C:\Projects\webbrain)             │
└────────────────────────────────────────────────────────┘
```

### The Coding Loop

1. **Inspect Status**: Call `workspace_status` to confirm the authorized root and active capabilities.
2. **Search Before Reading**: Use `workspace_search_code` to locate exact definitions, symbols, or files.
3. **Read Narrow Ranges**: Use `workspace_read_range` to fetch specific line ranges with revision numbers and content hashes.
4. **Apply Targeted Patch**: Apply changes with `workspace_apply_patch`, providing `expected_revision` to prevent overwriting concurrent edits.
5. **Inspect Git Diff**: Immediately call `workspace_git_diff` to review modifications and verify hunks.
6. **Focused Validation**: If authorized, run the narrowest relevant test or linter via `workspace_run_command`.
7. **Handle Revision Conflicts**: If an edit is rejected due to an external change (`REVISION_CONFLICT`), re-read the range and re-apply cleanly.

---

## 2. Security Model

Local filesystem access is a critical security boundary. The Workspace Bridge enforces defense-in-depth:

### 2.1 Authorized Root Boundary & Confinement
- **Single Root Invariant**: A daemon session binds to exactly one user-authorized canonical absolute directory. The model cannot supply an arbitrary absolute path to escape this root.
- **Canonicalization via `dunce`**: Uses the `dunce` crate to canonicalize Windows paths without prepending verbatim UNC prefixes (`\\?\C:\`), ensuring reliable string and prefix checks.
- **Traversal Rejection**: Relative paths containing `..` or leading slashes are rejected. After resolving, target paths must strictly start with the authorized root.
- **Symlink & Junction Defense**: Windows junctions, NTFS reparse points, and symlinks are resolved to their target destination. Any link pointing outside the authorized root is rejected with `PATH_OUTSIDE_WORKSPACE`.
- **Reserved DOS Device Names**: Rejects Windows reserved names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`, with or without extensions) to prevent hangs or denial-of-service.
- **Binary File Quarantine**: Edit and read tools only operate on text files. The first 8 KB of any opened file are inspected for null bytes (`0x00`) or invalid UTF-8.

### 2.2 Localhost Pairing & Handshake
- **Loopback-Only Binding**: The daemon strictly binds to `127.0.0.1` (or `::1`). It never listens on LAN interfaces or `0.0.0.0`.
- **Pairing Secret / Token**: Requires an authentication token exchanged during `auth.handshake`.
- **Origin Verification**: If the WebSocket request contains an HTTP Origin header, it must match `chrome-extension://<extension-id>` or `null`/native. Web page origins (`http://...`, `https://...`) are rejected with HTTP 403 / close code 1008 (`Untrusted Origin`).

### 2.3 Capability Grants
Connecting a workspace does not grant blanket write or shell access:
- **Workspace Read / Search**: Enabled by default in Ask, Act, and Dev modes once connected.
- **Workspace Write / Patch**: Gated by `Capability.WORKSPACE_WRITE`. Requires user opt-in in Settings ("Allow file edits").
- **Workspace Command Execution**: Gated by `Capability.WORKSPACE_COMMAND`. Disabled by default; requires explicit opt-in ("Allow terminal commands").

### 2.4 Prompt Injection Defense (`UNTRUSTED_CONTENT_TOOLS`)
All workspace outputs—file contents, diffs, search results, and command stdout/stderr—carry user/attacker-controllable text.
- All 7 workspace tools are registered in `UNTRUSTED_CONTENT_TOOLS` in `permission-gate.js`.
- Outputs are sealed with `<untrusted_page_content id="<nonce>">` before entering LLM context.
- Breakout attempts and fake boundary tags inside source files are neutralized.

### 2.5 Atomic Writes & Lock Retries
- Atomic file writes are staged in a sibling temporary file (`.wb-tmp-XXXXXX`) in the same directory and atomically renamed onto the target file.
- Line endings (CRLF vs LF) and UTF-8 Byte Order Marks (BOM) are detected and preserved.
- Windows file lock retries (exponential backoff up to 3 retries, 50ms) accommodate transient locks from Windows Defender or active editors.

---

## 3. Protocol v1 Specification

Communication between the Chrome extension and the Rust daemon uses JSON-RPC over a persistent WebSocket.

### 3.1 Request Envelope
```json
{
  "v": 1,
  "id": "req_123",
  "method": "workspace.read_range",
  "params": {
    "path": "src/main.rs",
    "startLine": 1,
    "endLine": 30
  }
}
```

### 3.2 Response Envelope
- **Success**:
```json
{
  "v": 1,
  "id": "req_123",
  "ok": true,
  "result": {
    "path": "src/main.rs",
    "revision": 3,
    "hash": "8a3f91...",
    "content": "fn main() {\n    println!(\"Hello\");\n}\n",
    "startLine": 1,
    "endLine": 30,
    "totalLines": 3
  }
}
```
- **Error**:
```json
{
  "v": 1,
  "id": "req_123",
  "ok": false,
  "error": {
    "code": "REVISION_CONFLICT",
    "message": "File changed externally (expected rev 3, current rev 4)"
  }
}
```

### 3.3 Server Push Events
```json
{
  "v": 1,
  "event": "file.changed",
  "data": {
    "path": "src/main.rs",
    "revision": 4,
    "hash": "9b4e02..."
  }
}
```

### 3.4 Standard Error Codes
| Code | Description |
|---|---|
| `UNAUTHENTICATED` | Handshake missing, token invalid, or origin rejected. |
| `PROTOCOL_MISMATCH` | Client protocol version incompatible with daemon. |
| `WORKSPACE_NOT_AUTHORIZED` | Workspace root not set or permission denied. |
| `PATH_OUTSIDE_WORKSPACE` | Attempted path traversal or symlink escape outside root. |
| `NOT_FOUND` | Target file or directory does not exist. |
| `NOT_TEXT_FILE` | Binary file detected. |
| `FILE_TOO_LARGE` | File exceeds maximum buffer cap. |
| `REVISION_CONFLICT` | File modified externally since last read; edit rejected. |
| `PATCH_REJECTED` | Patch context does not match or patch syntax invalid. |
| `COMMAND_NOT_ALLOWED` | Command execution capability disabled. |
| `COMMAND_TIMEOUT` | Command exceeded deadline. |
| `RESULT_TRUNCATED` | Payload exceeded character/line caps. |
| `INTERNAL_ERROR` | Unexpected daemon error. |

---

## 4. Setup & Developer Guide

### 4.1 Prerequisites
- **Node.js**: v20+ (tested on Node v24).
- **Rust Toolchain**: `rustc` and `cargo` 1.80+ (tested on 1.98.1 on Windows 11).
- **Browser**: Google Chrome or Chromium (Manifest V3) or Firefox (MV2).

### 4.2 Building the Rust Daemon
```bash
cd workspace-bridge
cargo build --release
```
The compiled executable is located at `workspace-bridge/target/release/webbrain-workspace.exe` (or `webbrain-workspace` on Unix).

### 4.3 Running the Daemon
```bash
# Basic launch with default port (18374)
./target/release/webbrain-workspace serve --root "C:\Users\miski\Desktop\my-project"

# Launch with custom port and pairing token
./target/release/webbrain-workspace serve \
  --root "C:\Users\miski\Desktop\my-project" \
  --port 18374 \
  --token "my-secret-token" \
  --allow-write \
  --allow-command
```

CLI options:
- `--root <PATH>`: Project directory to serve (required).
- `--port <PORT>`: WebSocket port (default: 18374).
- `--token <STRING>`: Authentication token (auto-generated if omitted).
- `--allow-write`: Enable file editing (default: true).
- `--allow-command`: Enable command execution (default: false).

### 4.4 Connecting the WebBrain Extension
1. Open the WebBrain extension Settings (`chrome-extension://<id>/src/ui/settings.html`).
2. Scroll to the **Local Workspace Bridge** section.
3. Toggle **Local Workspace Bridge** ON.
4. Set the **Bridge URL** (default: `ws://127.0.0.1:18374`).
5. Enter the **Pairing Token** (if configured on the daemon).
6. Configure permission toggles:
   - **Allow file edits**: Enables `workspace_apply_patch`.
   - **Allow terminal commands**: Enables `workspace_run_command`.
7. The status indicator will turn green: `Connected: <rootName>`.
8. In the WebBrain Sidepanel, a workspace badge will appear in the header showing the connected project.

### 4.5 Troubleshooting
- **Cannot connect / "Connection refused"**: Verify the daemon is running (`webbrain-workspace.exe serve`) and listening on `127.0.0.1:18374`.
- **403 / "Untrusted Origin"**: Occurs if connecting from a web page rather than the WebBrain extension or localhost tool.
- **UNAUTHENTICATED / "Invalid pairing token"**: Ensure the token entered in Settings matches the daemon's `--token` argument.
- **Port Conflict**: If port 18374 is busy, start the daemon with `--port 18375` and update the URL in Settings to `ws://127.0.0.1:18375`.

---

## 5. Maintenance Contract & Upstream Resilience

The Workspace Bridge is designed to survive upstream WebBrain updates without breaking or requiring core rewrites.

### Minimal Core Integration Seams
The workspace capability touches WebBrain core only at small, well-defined adapter seams:
1. `src/chrome/src/agent/workspace-tools.js` / `src/firefox/src/agent/workspace-tools.js`: Standalone tool schemas and prompt guidance.
2. `src/chrome/src/agent/tools.js` / `src/firefox/src/agent/tools.js`: Dynamic tool exposure hook (~15 lines in `getToolsForMode`).
3. `src/chrome/src/agent/permission-gate.js` / `src/firefox/src/agent/permission-gate.js`: Capability definitions and untrusted content classifications.
4. `src/chrome/src/offscreen/offscreen.html`: Single `<script src="workspace-bridge.js"></script>` inclusion.
5. `src/chrome/src/offscreen/workspace-bridge.js`: Standalone WebSocket client in the offscreen host.
6. `src/chrome/src/workspace-runs.js`: Standalone background workspace controller.
7. `src/chrome/src/agent/agent.js`: Minimal dispatch delegation (~10 lines in `_executeToolImpl` and `_buildSystemPrompt`).
8. `src/chrome/src/background.js`: Minimal message routing for workspace actions.
9. `src/chrome/src/ui/settings.html` & `settings.js`: Workspace configuration section.

### Upstream Update Safety
- **No Overwriting of Existing Bridges**: The workspace bridge does not touch or modify the MCP / Cloud bridge (`cloud-bridge.js`, `cloud-runs.js`).
- **Clean Disconnect Fallback**: If the workspace daemon is absent or disconnected, the agent loop behaves identically to stock WebBrain. Zero workspace tools are advertised to the LLM, preserving stock tool counts and prompt structures.
- **Isolated Daemon Crate**: The Rust daemon lives in `workspace-bridge/` with zero dependencies on WebBrain internal JavaScript modules.
