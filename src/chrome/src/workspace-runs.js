/**
 * Background Workspace Manager & Controller.
 *
 * Manages workspace connection lifecycle, pairing token, active project root,
 * permissions, file revision caching, and tool execution dispatch for the
 * WebBrain agent.
 */

export const WORKSPACE_STORAGE_KEY = 'webbrainWorkspaceConfig';
export const DEFAULT_WORKSPACE_URL = 'ws://127.0.0.1:18374';

export function createWorkspaceManager({ chromeApi = chrome, ensureOffscreen }) {
  const config = {
    enabled: false,
    url: DEFAULT_WORKSPACE_URL,
    token: '',
    allowWrite: true,
    allowCommand: false,
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

  async function loadConfig() {
    try {
      const stored = await chromeApi.storage.local.get({
        [WORKSPACE_STORAGE_KEY]: {
          enabled: false,
          url: DEFAULT_WORKSPACE_URL,
          token: '',
          allowWrite: true,
          allowCommand: false,
        },
      });
      const c = stored[WORKSPACE_STORAGE_KEY] || {};
      config.enabled = c.enabled === true;
      config.url = c.url || DEFAULT_WORKSPACE_URL;
      config.token = c.token || '';
      config.allowWrite = c.allowWrite !== false;
      config.allowCommand = c.allowCommand === true;
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
      return getWorkspaceStatus();
    }
    return await connectWorkspace(config);
  }

  async function connectWorkspace(opts = {}) {
    if (opts.url) config.url = opts.url;
    if (opts.token !== undefined) config.token = opts.token;
    if (opts.allowWrite !== undefined) config.allowWrite = opts.allowWrite === true;
    if (opts.allowCommand !== undefined) config.allowCommand = opts.allowCommand === true;
    config.enabled = true;

    await saveConfig();

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
      return getWorkspaceStatus();
    } catch (e) {
      state.connected = false;
      state.authenticated = false;
      state.lastError = e.message || String(e);
      return getWorkspaceStatus();
    }
  }

  async function disconnectWorkspace() {
    config.enabled = false;
    await saveConfig();

    try {
      if (typeof ensureOffscreen === 'function') {
        await ensureOffscreen();
      }
      await chromeApi.runtime.sendMessage({
        action: 'workspace_bridge_stop',
      });
    } catch {}

    state.connected = false;
    state.authenticated = false;
    state.sessionId = null;
    state.root = null;
    state.rootName = null;
    state.lastError = '';
    fileRevisionCache.clear();

    return getWorkspaceStatus();
  }

  async function getWorkspaceStatus() {
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
    return state.connected === true && state.authenticated === true;
  }

  function canWrite() {
    return isConnected() && config.allowWrite === true;
  }

  function canCommand() {
    return isConnected() && config.allowCommand === true;
  }

  function root() {
    return state.root;
  }

  function rootName() {
    return state.rootName;
  }

  async function executeWorkspaceTool(name, args = {}) {
    if (!isConnected()) {
      return {
        success: false,
        error: 'Workspace bridge is not connected. Connect a local workspace in Settings to use workspace tools.',
      };
    }

    // Permission checks
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

    if (name === 'coding_delegate') {
      return await codingClientV2.startTask({
        summary: args.summary,
        instructions: args.instructions,
        browserObservations: args.browser_observations || args.browserObservations,
        verificationGoal: args.verification_goal || args.verificationGoal,
      });
    }
    if (name === 'coding_steer') {
      return await codingClientV2.steerTask(args.message);
    }
    if (name === 'coding_status') {
      return await codingClientV2.getTaskStatus();
    }
    if (name === 'coding_abort') {
      return await codingClientV2.abortTask();
    }

    switch (name) {
      case 'workspace_status':
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
          content: String(args.content ?? ''),
          overwrite: args.overwrite === true,
        };
        break;


      case 'workspace_git_diff':
        method = 'workspace.git_diff';
        params = {
          paths: Array.isArray(args.paths) ? args.paths : (args.paths ? [args.paths] : undefined),
          maxBytes: Number(args.max_bytes ?? args.maxBytes) || undefined,
        };
        break;

      case 'workspace_run_command':
        method = 'workspace.run_command';
        timeoutMs = (Number(args.timeout_seconds ?? args.timeoutSeconds) || 30) * 1000;
        params = {
          command: String(args.command || ''),
          args: Array.isArray(args.args) ? args.args : undefined,
          timeoutMs,
        };
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

      if (!res?.ok) {
        return {
          success: false,
          error: res?.error || 'Workspace tool call failed.',
          code: res?.code || 'INTERNAL_ERROR',
          details: res?.details,
        };
      }

      const result = res.result || {};

      // Cache updated file revisions
      if ((name === 'workspace_read_file' || name === 'workspace_read_range') && result.path && result.revision != null) {
        fileRevisionCache.set(result.path, {
          revision: result.revision,
          hash: result.hash || '',
          timestamp: Date.now(),
        });
      } else if (name === 'workspace_apply_patch' && result.path && result.newRevision != null) {
        fileRevisionCache.set(result.path, {
          revision: result.newRevision,
          hash: result.newHash || '',
          timestamp: Date.now(),
        });
      } else if (name === 'workspace_create_file' && result.path) {
        fileRevisionCache.delete(result.path);
        if (result.revision != null) {
          fileRevisionCache.set(result.path, {
            revision: result.revision,
            hash: result.hash || '',
            timestamp: Date.now(),
          });
        }
      }

      return {
        success: true,
        ...result,
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
