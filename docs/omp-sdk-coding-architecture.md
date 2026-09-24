# WebBrain OMP SDK Coding Architecture (V2)

## Overview
WebBrain V2 migrates workspace codebase interactions from low-level Rust workspace bridge JSON-RPC primitives to task-oriented in-process OMP SDK coding sessions (`omp-sdk-v2`).

## Separation of Concerns
1. **WebBrain Browser Specialist**:
   - DOM inspection, clicks, typing, navigation, network sniffing, console capturing, screenshots, and browser verification loops.
2. **OMP SDK Coding Specialist**:
   - Codebase search (`grep`, `glob`), file reading (`read`), editing (`edit`), LSP diagnostics, test/build execution (`bash`), and coding agent loops.

## Wire Protocol (`/webbrain/coding`)
- Localhost WebSocket (`ws://127.0.0.1:18374/webbrain/coding`).
- **Client -> Host Messages**: `hello`, `workspace.open`, `coding.start`, `coding.steer`, `coding.follow_up`, `coding.abort`, `coding.status`, `session.close`, `verification.result`.
- **Host -> Client Events**: `hello.ok`, `workspace.opened`, `coding.started`, `coding.progress`, `coding.tool_activity`, `coding.changed_files`, `coding.verification_requested`, `coding.completed`, `coding.failed`, `coding.aborted`, `session.closed`, `host.error`.

## High-Level Handoff Tools
- `coding_delegate`: Delegates coding tasks with user intent and compact browser observations.
- `coding_steer`: Steers active coding tasks with verification failure feedback.
- `coding_status`: Checks task progress.
- `coding_abort`: Cancels active coding tasks.

Low-level `workspace_*` primitives are hidden from the model in V2 mode and retained only in `rust-v1` fallback mode.
