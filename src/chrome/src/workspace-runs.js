/**
 * Background Workspace Manager & Controller.
 *
 * Supports dual backend architecture:
 * - OMP SDK V2 (`omp-sdk-v2` - developer default) connecting directly to in-process coding worker
 * - Rust Daemon V1 (`rust-v1` - fallback baseline) connecting via offscreen WebSocket
 */

import { createCodingClientV2 } from './agent/coding-client-v2.js';

export const WORKSPACE_STORAGE_KEY = 'webbrainWorkspaceConfig';
export const DEFAULT_WORKSPACE_URL = 'ws://127.0.0.1:18374/webbrain/coding';
export const DEFAULT_RUST_V1_URL = 'ws://127.0.0.1:18374';

export function createWorkspaceManager({ chromeApi = chrome, ensureOffscreen } = {}) {
  const codingClientV2 = createCodingClientV2({ chromeApi });

  const config = {
    enabled: false,
    url: DEFAULT_WORKSPACE_URL,
    token: '',
    allowWrite: true,
    allowCommand: false,
    workspaceBackend: 'omp-sdk-v2', // Developer default V2
    workspacePath: '',
  };

  const state = {
    connected: false,
    authenticated: false,
    sessionId: null,
    root: null,
    rootName: null,
    capabilities: [],
    git: false,
    watcherHealthy: false,
    lastError: '',
  };

  // Cache: relativePath -> { revision: number, hash: string, timestamp: number }
  const fileRevisionCache = new Map();

  // Wire V2 event listeners to forward events to sidepanel / background
  codingClientV2.addEventListener((event, data) => {
    try {
      chromeApi.runtime.sendMessage({
        target: 'sidepanel',
        action: 'workspace_event',
        event,
        data,
      }).catch(() => {});
    } catch {}
  });

  async function loadConfig() {
    try {
      const stored = await chromeApi.storage.local.get({
        [WORKSPACE_STORAGE_KEY]: {
          enabled: false,
          url: DEFAULT_WORKSPACE_URL,
          token: '',
          allowWrite: true,
          allowCommand: false,
          workspaceBackend: 'omp-sdk-v2',
          workspacePath: '',
        },
      });
      const c = stored[WORKSPACE_STORAGE_KEY] || {};
      config.enabled = c.enabled === true;
      config.url = c.url || (c.workspaceBackend === 'rust-v1' ? DEFAULT_RUST_V1_URL : DEFAULT_WORKSPACE_URL);
      config.token = c.token || '';
      config.allowWrite = c.allowWrite !== false;
      config.allowCommand = c.allowCommand === true;
      config.workspaceBackend = c.workspaceBackend || 'omp-sdk-v2';
      config.workspacePath = c.workspacePath || '';
    } catch (e) {
      console.warn('[WebBrain Workspace] Error loading stored config:', e);
    }
  }

  async function saveConfig() {
    try {
      await chromeApi.storage.local.set({
        [WORKSPACE_STORAGE_KEY]: {
          enabled: config.enabled,
          url: config.url,
          token: config.token,
          allowWrite: config.allowWrite,
          allowCommand: config.allowCommand,
          workspaceBackend: config.workspaceBackend,
          workspacePath: config.workspacePath,
        },
      });
    } catch (e) {
      console.warn('[WebBrain Workspace] Error saving config:', e);
    }
  }

  async function syncBridge() {
    await loadConfig();
    if (!config.enabled) {
      state.connected = false;
      state.authenticated = false;
      state.sessionId = null;
      state.root = null;
      return await getWorkspaceStatus();
    }
    return await connectWorkspace(config);
  }

  async function connectWorkspace(opts = {}) {
    if (opts.url) config.url = opts.url;
    if (opts.token !== undefined) config.token = opts.token;
    if (opts.allowWrite !== undefined) config.allowWrite = opts.allowWrite === true;
    if (opts.allowCommand !== undefined) config.allowCommand = opts.allowCommand === true;
    if (opts.workspaceBackend) {
      config.workspaceBackend = opts.workspaceBackend;
    } else if (opts.url) {
      const trimmedUrl = opts.url.trim();
      if (trimmedUrl.includes('/webbrain/coding')) {
        config.workspaceBackend = 'omp-sdk-v2';
      } else if (trimmedUrl === DEFAULT_RUST_V1_URL || trimmedUrl === 'ws://127.0.0.1:18374' || trimmedUrl === 'ws://localhost:18374') {
        config.workspaceBackend = 'rust-v1';
      }
    } else if (ensureOffscreen) {
      config.workspaceBackend = 'rust-v1';
    }
    if (opts.path) config.workspacePath = opts.path;
    config.enabled = true;

    await saveConfig();

    if (config.workspaceBackend === 'omp-sdk-v2') {
      const targetPath = config.workspacePath || opts.workspacePath || opts.path || '';
      if (!targetPath || targetPath === '.' || !(/^[a-zA-Z]:[/\\]|^\//.test(targetPath))) {
        state.connected = false;
        state.authenticated = false;
        state.lastError = 'Workspace project root must be an absolute path (e.g. C:\\Projects\\app)';
        return await getWorkspaceStatus();
      }

      try {
        await codingClientV2.connect({ url: config.url, token: config.token, timeoutMs: opts.timeoutMs || 12000 });
        const openRes = await codingClientV2.openWorkspace(targetPath);
        state.connected = true;
        state.authenticated = true;
        state.root = openRes.path || openRes.rootPath || targetPath;
        state.rootName = openRes.rootName || (state.root ? state.root.split(/[/\\]/).pop() : 'workspace');
        state.lastError = '';

        try {
          const status = await getWorkspaceStatus();
          chromeApi.runtime.sendMessage({
            action: 'workspace_status_changed',
            status,
          }).catch(() => {});
        } catch {}

        return await getWorkspaceStatus();
      } catch (e) {
        state.connected = false;
        state.authenticated = false;
        state.lastError = e.message || String(e);
        return await getWorkspaceStatus();
      }
    }

    // Rust V1 fallback connection flow via offscreen document
    try {
      if (typeof ensureOffscreen === 'function') {
        await ensureOffscreen();
      }

      const res = await chromeApi.runtime.sendMessage({
        action: 'workspace_bridge_start',
        url: config.url,
        token: config.token,
      });

      if (res?.status) {
        state.connected = res.status.connected === true;
        state.authenticated = res.status.authenticated === true;
        state.lastError = res.status.lastError || '';
        if (res.status.session) {
          applySession(res.status.session);
        }
      }
      return await getWorkspaceStatus();
    } catch (e) {
      state.connected = false;
      state.authenticated = false;
      state.lastError = e.message || String(e);
      return await getWorkspaceStatus();
    }
  }

  async function disconnectWorkspace() {
    config.enabled = false;
    await saveConfig();

    if (config.workspaceBackend === 'omp-sdk-v2') {
      try {
        await codingClientV2.disconnect();
      } catch {}
    } else {
      try {
        if (typeof ensureOffscreen === 'function') {
          await ensureOffscreen();
        }
        await chromeApi.runtime.sendMessage({
          action: 'workspace_bridge_stop',
        });
      } catch {}
    }

    state.connected = false;
    state.authenticated = false;
    state.sessionId = null;
    state.root = null;
    state.rootName = null;
    state.lastError = '';
    fileRevisionCache.clear();

    try {
      const status = await getWorkspaceStatus();
      chromeApi.runtime.sendMessage({
        action: 'workspace_status_changed',
        status,
      }).catch(() => {});
    } catch {}

    return await getWorkspaceStatus();
  }

  async function getWorkspaceStatus() {
    if (config.workspaceBackend === 'omp-sdk-v2') {
      const v2Status = codingClientV2.getStatus();
      return {
        enabled: config.enabled,
        connected: v2Status.connected,
        authenticated: v2Status.authenticated,
        backend: 'omp-sdk-v2',
        sessionId: state.sessionId,
        root: state.root || v2Status.root,
        rootName: state.rootName || v2Status.rootName,
        guidance: "Connected OMP SDK V2 coding worker. Use coding_delegate to delegate software engineering tasks.",
        capabilities: ['read', 'write', 'command', 'coding_delegate'],
        allowWrite: config.allowWrite,
        allowCommand: config.allowCommand,
        git: true,
        watcherHealthy: true,
        lastError: state.lastError,
        v2: v2Status,
      };
    }

    try {
      if (typeof ensureOffscreen === 'function') {
        await ensureOffscreen();
      }
      const res = await chromeApi.runtime.sendMessage({
        action: 'workspace_bridge_status',
      });
      if (res?.status) {
        state.connected = res.status.connected === true;
        state.authenticated = res.status.authenticated === true;
        state.lastError = res.status.lastError || state.lastError;
        if (res.status.session) {
          applySession(res.status.session);
        }
      }
    } catch {}

    return {
      enabled: config.enabled,
      url: config.url,
      connected: state.connected,
      authenticated: state.authenticated,
      backend: 'rust-v1',
      sessionId: state.sessionId,
      root: state.root,
      rootName: state.rootName,
      guidance: "Workspace root is '" + (state.rootName || 'project') + "'. Use '.' for this root directory.",
      capabilities: state.capabilities,
      allowWrite: config.allowWrite,
      allowCommand: config.allowCommand,
      git: state.git,
      watcherHealthy: state.watcherHealthy,
      lastError: state.lastError,
    };
  }

  function applySession(session) {
    if (!session) return;
    if (session.sessionId) state.sessionId = session.sessionId;
    if (session.root) state.root = session.root;
    if (session.rootName) state.rootName = session.rootName;
    if (Array.isArray(session.capabilities)) state.capabilities = session.capabilities;
    if (typeof session.git === 'boolean') state.git = session.git;
  }

  function handleWorkspaceEvent(event, data = {}) {
    if (event === 'file.changed' && data.path) {
      if (data.revision != null) {
        fileRevisionCache.set(data.path, {
          revision: data.revision,
          hash: data.hash || null,
          timestamp: Date.now(),
        });
      }
    }

    // Forward event to UI sidepanel
    try {
      chromeApi.runtime.sendMessage({
        target: 'sidepanel',
        action: 'workspace_event',
        event,
        data,
      }).catch(() => {});
    } catch {}
  }

  async function handleWorkspaceConnected(session) {
    state.connected = true;
    state.authenticated = true;
    state.lastError = '';
    applySession(session);
    try {
      const status = await getWorkspaceStatus();
      chromeApi.runtime.sendMessage({
        action: 'workspace_status_changed',
        status,
      }).catch(() => {});
    } catch {}
  }

  async function handleWorkspaceDisconnected() {
    state.connected = false;
    state.authenticated = false;
    state.sessionId = null;
    fileRevisionCache.clear();
    try {
      const status = await getWorkspaceStatus();
      chromeApi.runtime.sendMessage({
        action: 'workspace_status_changed',
        status,
      }).catch(() => {});
    } catch {}
  }

  function isConnected() {
    if (config.workspaceBackend === 'omp-sdk-v2') {
      return codingClientV2.isConnected();
    }
    return state.connected === true && state.authenticated === true;
  }

  function canWrite() {
    return isConnected() && config.allowWrite === true;
  }

  function canCommand() {
    return isConnected() && config.allowCommand === true;
  }

  function root() {
    if (config.workspaceBackend === 'omp-sdk-v2') {
      return state.root || codingClientV2.getStatus().root;
    }
    return state.root;
  }

  function rootName() {
    if (config.workspaceBackend === 'omp-sdk-v2') {
      return state.rootName || codingClientV2.getStatus().rootName;
    }
    return state.rootName;
  }

  async function executeWorkspaceTool(name, args = {}) {
    // 1. workspace_status immediately answers directly without falling into bridge call
    if (name === 'workspace_status') {
      return {
        success: true,
        result: await getWorkspaceStatus(),
      };
    }

    if (!isConnected()) {
      return {
        success: false,
        error: 'Workspace bridge is not connected. Connect a local workspace in Settings to use workspace tools.',
      };
    }

    // 2. OMP SDK V2 routing
    if (config.workspaceBackend === 'omp-sdk-v2') {
      if (name === 'coding_delegate') {
        const res = await codingClientV2.startTask({
          summary: args.summary,
          instructions: args.instructions,
          browserObservations: args.browser_observations || args.browserObservations,
          verificationGoal: args.verification_goal || args.verificationGoal,
        });
        return { success: true, ...(typeof res === 'object' && res !== null ? res : {}), result: res };
      }
      if (name === 'coding_steer') {
        const res = await codingClientV2.steerTask(args);
        return { success: true, ...(typeof res === 'object' && res !== null ? res : {}), result: res };
      }
      if (name === 'coding_status') {
        const res = await codingClientV2.getTaskStatus();
        return { success: true, ...(typeof res === 'object' && res !== null ? res : {}), result: res };
      }
      if (name === 'coding_abort') {
        const res = await codingClientV2.abortTask();
        return { success: true, ...(typeof res === 'object' && res !== null ? res : {}), result: res };
      }
      return {
        success: false,
        error: `Tool ${name} is a V1 primitive; in OMP SDK V2 mode use coding_delegate instead.`,
      };
    }

    // 3. Rust V1 fallback routing
    if (name.startsWith('coding_')) {
      return {
        success: false,
        error: `${name} requires OMP SDK V2 mode; current backend is rust-v1.`,
      };
    }

    if ((name === 'workspace_apply_patch' || name === 'workspace_create_file') && !canWrite()) {
      return {
        success: false,
        error: `Workspace write capability is not authorized. Enable "Allow file edits" in Settings to ${name === 'workspace_create_file' ? 'create files' : 'apply patches'}.`,
      };
    }

    if (name === 'workspace_run_command' && !canCommand()) {
      return {
        success: false,
        error: 'Workspace command execution is not authorized. Enable "Allow terminal commands" in Settings to run commands.',
      };
    }

    let method;
    let params;
    let timeoutMs;

    switch (name) {
      case 'workspace_search_code':
        method = 'workspace.search_code';
        params = {
          query: String(args.query || ''),
          limit: Number(args.limit) || 30,
          isRegex: args.is_regex ?? args.isRegex ?? false,
          caseSensitive: args.case_sensitive ?? args.caseSensitive ?? false,
          include: Array.isArray(args.include) ? args.include : undefined,
          exclude: Array.isArray(args.exclude) ? args.exclude : undefined,
          contextLines: Number(args.context_lines ?? args.contextLines) || undefined,
        };
        break;

      case 'workspace_read_file':
        method = 'workspace.read_file';
        params = {
          path: String(args.path || ''),
          maxChars: Number(args.max_chars ?? args.maxChars) || undefined,
        };
        break;

      case 'workspace_read_range':
        method = 'workspace.read_range';
        params = {
          path: String(args.path || ''),
          startLine: Number(args.start_line ?? args.startLine) || 1,
          endLine: Number(args.end_line ?? args.endLine) || 100,
        };
        break;

      case 'workspace_list_dir':
        method = 'workspace.list_dir';
        params = {
          path: args.path !== undefined && args.path !== null ? String(args.path) : '.',
          maxEntries: Number(args.max_entries ?? args.maxEntries) || 200,
        };
        break;

      case 'workspace_glob':
        method = 'workspace.glob';
        params = {
          pattern: String(args.pattern || ''),
          path: args.path !== undefined && args.path !== null ? String(args.path) : '.',
          maxMatches: Number(args.max_matches ?? args.maxMatches) || 200,
        };
        break;

      case 'workspace_apply_patch':
        method = 'workspace.apply_patch';
        params = {
          path: String(args.path || ''),
          expectedRevision: Number(args.expected_revision ?? args.expectedRevision) || 0,
          expectedHash: args.expected_hash ?? args.expectedHash ?? undefined,
          patch: args.patch || undefined,
          oldText: args.old_text ?? args.oldText ?? undefined,
          newText: args.new_text ?? args.newText ?? undefined,
        };
        break;

      case 'workspace_create_file':
        method = 'workspace.create_file';
        params = {
          path: String(args.path || ''),
          content: String(args.content || ''),
          overwrite: args.overwrite === true,
        };
        break;

      case 'workspace_git_diff':
        method = 'workspace.git_diff';
        params = {
          paths: Array.isArray(args.paths) ? args.paths : undefined,
          maxBytes: Number(args.max_bytes ?? args.maxBytes) || undefined,
        };
        break;

      case 'workspace_run_command':
        method = 'workspace.run_command';
        params = {
          command: String(args.command || ''),
          args: Array.isArray(args.args) ? args.args : undefined,
          timeoutMs: Number(args.timeout_ms ?? args.timeoutMs) || undefined,
        };
        timeoutMs = params.timeoutMs ? params.timeoutMs + 2000 : 35000;
        break;

      default:
        return {
          success: false,
          error: `Unknown workspace tool: ${name}`,
        };
    }

    try {
      if (typeof ensureOffscreen === 'function') {
        await ensureOffscreen();
      }

      const res = await chromeApi.runtime.sendMessage({
        action: 'workspace_bridge_call',
        method,
        params,
        timeoutMs,
      });

      if (res && res.error) {
        return {
          success: false,
          error: res.error.message || String(res.error),
          code: res.error.code,
        };
      }

      const result = res?.result !== undefined ? res.result : res;

      if (name === 'workspace_read_file' || name === 'workspace_read_range') {
        if (result?.path && result?.revision != null) {
          fileRevisionCache.set(result.path, {
            revision: result.revision,
            hash: result.hash || null,
            timestamp: Date.now(),
          });
        }
      } else if (name === 'workspace_apply_patch') {
        if (result?.path && result?.newRevision != null) {
          fileRevisionCache.set(result.path, {
            revision: result.newRevision,
            hash: result.newHash || null,
            timestamp: Date.now(),
          });
        }
      } else if (name === 'workspace_create_file') {
        if (result?.path && result?.revision != null) {
          fileRevisionCache.set(result.path, {
            revision: result.revision,
            hash: result.hash || null,
            timestamp: Date.now(),
          });
        }
      }

      return {
        success: true,
        ...(typeof result === 'object' && result !== null ? result : {}),
        result,
      };
    } catch (err) {
      return {
        success: false,
        error: err.message || String(err),
      };
    }
  }

  return {
    connectWorkspace,
    disconnectWorkspace,
    getWorkspaceStatus,
    executeWorkspaceTool,
    executeTool: executeWorkspaceTool,
    workspaceBackend: () => config.workspaceBackend,
    getConfig: () => ({ ...config }),
    saveConfig,
    loadConfig,
    handleWorkspaceEvent,
    handleWorkspaceConnected,
    handleWorkspaceDisconnected,
    isConnected,
    root,
    rootName,
    canWrite,
    canCommand,
    syncBridge,
    fileRevisionCache,
  };
}
