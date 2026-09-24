/**
 * OMP SDK Coding Client V2 for WebBrain.
 * Connects via localhost WebSocket to /webbrain/coding on the OMP SDK host.
 * Implements task-oriented coding handoff protocol v2, event normalization,
 * connection epochs, task retention until verification, and explicit fallback telemetry.
 */

export function createCodingClientV2({ chromeApi = globalThis.chrome || {} } = {}) {
  let ws = null;
  let connected = false;
  let authenticated = false;
  let bridgeUrl = 'ws://127.0.0.1:18374/webbrain/coding';
  let pairingToken = '';
  let connectionEpoch = 0;
  let currentWorkspaceId = null;
  let activeTaskId = null;
  let activeSessionId = null;
  let taskStatus = 'idle';
  let lastProgressState = 'idle';
  let rootPath = '';
  let rootName = '';
  let activeTools = [];
  let capabilities = { read: true, write: true, command: true };
  let pendingRequests = new Map();
  let eventListeners = new Set();
  let fallbackUsed = false;

  function generateId(prefix = 'req') {
    return `${prefix}-${Math.random().toString(36).substring(2, 9)}`;
  }

  function notifyListeners(event, data) {
    for (const listener of eventListeners) {
      try {
        listener(event, data);
      } catch {}
    }
  }

  function clearPendingRequests(error = new Error('Connection closed or reconnected')) {
    for (const [id, req] of pendingRequests.entries()) {
      try {
        req.reject(error);
      } catch {}
    }
    pendingRequests.clear();
  }

  function getStatus() {
    return {
      backend: 'omp-sdk-v2',
      connected,
      authenticated,
      bridgeUrl,
      root: rootPath,
      rootName,
      activeTools,
      capabilities,
      taskStatus,
      lastProgressState,
      activeTaskId,
      activeSessionId,
      connectionEpoch,
      fallbackUsed,
    };
  }

  async function connect(config = {}) {
    bridgeUrl = config.url || bridgeUrl;
    pairingToken = config.token || pairingToken;
    connectionEpoch++;
    const epoch = connectionEpoch;

    if (ws) {
      try { ws.close(); } catch {}
      ws = null;
    }
    clearPendingRequests(new Error('Reconnecting V2 coding client'));

    return new Promise((resolve, reject) => {
      try {
        ws = new WebSocket(bridgeUrl);
      } catch (err) {
        connected = false;
        authenticated = false;
        reject(err);
        return;
      }

      const timeout = setTimeout(() => {
        if (!authenticated && epoch === connectionEpoch) {
          try { ws.close(); } catch {}
          reject(new Error('OMP SDK V2 coding connection timeout after 10000ms'));
        }
      }, 10000);

      ws.onopen = () => {
        connected = true;
        const helloId = generateId('hello');
        const helloMsg = {
          v: 2,
          id: helloId,
          type: 'hello',
          client: {
            name: 'webbrain-extension',
            version: '36.8.0',
          },
          token: pairingToken || undefined,
        };

        pendingRequests.set(helloId, {
          resolve: (res) => {
            clearTimeout(timeout);
            authenticated = true;
            if (res.sessionId) activeSessionId = res.sessionId;
            resolve({ ok: true, status: getStatus(), session: res });
          },
          reject: (err) => {
            clearTimeout(timeout);
            connected = false;
            authenticated = false;
            reject(err);
          },
        });

        try {
          ws.send(JSON.stringify(helloMsg));
        } catch (err) {
          pendingRequests.delete(helloId);
          clearTimeout(timeout);
          reject(err);
        }
      };

      ws.onmessage = (event) => {
        try {
          const msg = JSON.parse(event.data);
          const { id, type, replyTo, ...payload } = msg;

          if (replyTo && pendingRequests.has(replyTo)) {
            const req = pendingRequests.get(replyTo);
            pendingRequests.delete(replyTo);
            if (type === 'host.error' || payload.error) {
              req.reject(new Error(payload.error || 'Unknown host error'));
            } else {
              req.resolve(payload);
            }
            return;
          }

          // Handle incoming host events
          handleHostEvent(type, payload);
        } catch (err) {
          console.error('[CodingClientV2] Message parse error:', err);
        }
      };

      ws.onerror = (err) => {
        connected = false;
        authenticated = false;
        notifyListeners('error', { error: err });
      };

      ws.onclose = () => {
        connected = false;
        authenticated = false;
        clearPendingRequests(new Error('V2 coding WebSocket closed'));
        notifyListeners('disconnected', {});
      };
    });
  }

  function handleHostEvent(type, payload) {
    switch (type) {
      case 'coding.started':
        if (payload.taskId) activeTaskId = payload.taskId;
        lastProgressState = 'starting';
        taskStatus = 'running';
        notifyListeners('started', payload);
        break;
      case 'coding.progress':
        lastProgressState = payload.state || 'running';
        taskStatus = payload.state === 'completed' ? 'completed' : payload.state === 'failed' ? 'failed' : 'running';
        notifyListeners('progress', payload);
        break;
      case 'coding.tool_activity':
        notifyListeners('tool_activity', payload);
        break;
      case 'coding.changed_files':
        if (payload.changedFiles) {
          notifyListeners('changed_files', payload);
        }
        break;
      case 'coding.verification_requested':
        taskStatus = 'ready_for_browser_verification';
        notifyListeners('verification_requested', payload);
        break;
      case 'coding.completed':
        taskStatus = 'completed';
        // Note: activeTaskId is purposely NOT cleared here so verification.result or follow-ups can reference it!
        notifyListeners('completed', payload);
        break;
      case 'coding.failed':
        taskStatus = 'failed';
        notifyListeners('failed', payload);
        break;
      case 'coding.aborted':
        taskStatus = 'aborted';
        activeTaskId = null;
        notifyListeners('aborted', payload);
        break;
      default:
        notifyListeners(type, payload);
        break;
    }
  }

  async function sendRequest(type, params, timeoutMs = 30000) {
    if (!ws || ws.readyState !== WebSocket.OPEN || !authenticated) {
      throw new Error('OMP SDK V2 coding service not connected or authenticated');
    }

    const id = generateId('req');
    const msg = { v: 2, id, type, ...params };

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pendingRequests.delete(id);
        reject(new Error(`Request ${type} timed out after ${timeoutMs}ms`));
      }, timeoutMs);

      pendingRequests.set(id, {
        resolve: (res) => {
          clearTimeout(timer);
          resolve(res);
        },
        reject: (err) => {
          clearTimeout(timer);
          reject(err);
        },
      });

      try {
        ws.send(JSON.stringify(msg));
      } catch (err) {
        pendingRequests.delete(id);
        clearTimeout(timer);
        reject(err);
      }
    });
  }

  async function openWorkspace(workspacePath) {
    const res = await sendRequest('workspace.open', { path: workspacePath });
    currentWorkspaceId = res.workspaceId || res.id || 'ws-default';
    rootPath = res.path || res.rootPath || workspacePath;
    rootName = res.rootName || rootPath.split(/[/\\]/).pop() || 'workspace';
    activeTools = res.activeTools || [];
    if (res.capabilities) capabilities = res.capabilities;
    return res;
  }

  async function startTask(taskSpec) {
    taskStatus = 'starting';
    const res = await sendRequest('coding.start', {
      workspaceId: currentWorkspaceId,
      task: taskSpec,
    });
    if (res.taskId) activeTaskId = res.taskId;
    if (res.sessionId) activeSessionId = res.sessionId;
    taskStatus = 'running';
    return res;
  }

  async function steerTask(messageOrSpec) {
    const instruction = typeof messageOrSpec === 'string' ? messageOrSpec : messageOrSpec?.instruction;
    const browserObservations = typeof messageOrSpec === 'object' ? messageOrSpec?.browserObservations : undefined;
    return await sendRequest('coding.steer', {
      taskId: activeTaskId,
      instruction,
      browserObservations,
    });
  }

  async function followUp(messageOrSpec) {
    const instruction = typeof messageOrSpec === 'string' ? messageOrSpec : messageOrSpec?.instruction;
    const browserObservations = typeof messageOrSpec === 'object' ? messageOrSpec?.browserObservations : undefined;
    return await sendRequest('coding.follow_up', {
      taskId: activeTaskId,
      instruction,
      browserObservations,
    });
  }

  async function abortTask() {
    try {
      const res = await sendRequest('coding.abort', { taskId: activeTaskId });
      taskStatus = 'aborted';
      activeTaskId = null;
      return res;
    } catch (err) {
      taskStatus = 'aborted';
      activeTaskId = null;
      throw err;
    }
  }

  async function getTaskStatus() {
    return await sendRequest('coding.status', { taskId: activeTaskId });
  }

  async function submitVerification(resultOrSpec) {
    const success = typeof resultOrSpec === 'boolean' ? resultOrSpec : resultOrSpec?.success === true;
    const feedback = typeof resultOrSpec === 'object' ? resultOrSpec?.feedback || resultOrSpec?.summary : undefined;
    const browserObservations = typeof resultOrSpec === 'object' ? resultOrSpec?.browserObservations : undefined;

    const res = await sendRequest('verification.result', {
      taskId: activeTaskId,
      success,
      feedback,
      browserObservations,
    });
    // Once verification is finalized, clear activeTaskId
    activeTaskId = null;
    return res;
  }

  async function closeSession() {
    try {
      if (activeSessionId) {
        await sendRequest('session.close', { workspaceId: currentWorkspaceId });
      }
    } catch {}
    if (ws) {
      try { ws.close(); } catch {}
      ws = null;
    }
    connected = false;
    authenticated = false;
    clearPendingRequests(new Error('Session closed'));
  }

  function addEventListener(fn) {
    eventListeners.add(fn);
    return () => eventListeners.delete(fn);
  }

  function markFallbackUsed(used = true) {
    fallbackUsed = used;
  }

  return {
    connect,
    disconnect: closeSession,
    getStatus,
    openWorkspace,
    startTask,
    steerTask,
    followUp,
    abortTask,
    getTaskStatus,
    submitVerification,
    addEventListener,
    markFallbackUsed,
    isConnected: () => connected && authenticated,
  };
}
