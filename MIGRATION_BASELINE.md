# MIGRATION BASELINE (Phase 0)

## Repositories & Environment
- **WebBrain Repo**: `C:\Users\miski\Desktop\webbrain`
- **Current Branch**: `feature/omp-sdk-coding-migration` (branched from `feature/workspace-bridge`)
- **Upstream Main**: `https://github.com/webbrain-one/webbrain.git` (`remotes/upstream/main`)
- **Git Status**: Clean working tree on migration branch (untracked migration plan added).
- **Existing OMP SDK App**: `portable-omp-gateway` at `C:\Users\miski\Desktop\api`
  - `@oh-my-pi/pi-coding-agent`: `18.2.5`
  - `@oh-my-pi/pi-ai`: `18.2.5`
  - Runtime: Bun (`v1.4.2`)

## V1 Components (WebBrain & Rust Workspace Bridge)
- **Rust Daemon (`workspace-bridge/`)**:
  - `server.rs`, `protocol.rs`, `auth.rs`, `paths.rs`, `files.rs`, `patch.rs`, `search.rs`, `watcher.rs`, `git.rs`, `command.rs`, `session.rs`, `lib.rs`, `main.rs`
- **Chrome Extension V1 Bridge**:
  - `src/chrome/src/offscreen/workspace-bridge.js`
  - `src/chrome/src/workspace-runs.js`
  - `src/chrome/src/agent/workspace-tools.js`
  - `src/chrome/src/agent/tools.js`
  - `src/chrome/src/agent/permission-gate.js`
  - `src/chrome/src/agent/agent.js`
  - `src/chrome/src/background.js`

## Baseline Test Results
1. **Rust Workspace Bridge Tests (`cargo test`)**: 69 tests passed successfully (8 suites).
2. **Security Injection Corpus (`npm run test:security`)**: 60/60 checks passed.
3. **Chrome Build (`npm run build:chrome`)**: Successfully built unpacked extension (`build\chrome/`).
4. **OMP Gateway Tests (`bun test` in `../api`)**: 8 tests passed, 3 skipped.

## Migration Goals (V2)
- Introduce in-process OMP SDK agent plane module in portable-omp-gateway (`/webbrain/coding`).
- Implement task-oriented protocol (`coding_delegate`, `coding_steer`, `coding_status`, `coding_abort`) over localhost WebSocket.
- Hide low-level `workspace_*` tools from model in V2 mode while preserving browser tools in WebBrain.
- Keep Rust V1 backend behind feature flag (`workspaceBackend = "rust-v1" | "omp-sdk-v2"`) for fallback and rollback validation.
