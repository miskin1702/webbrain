/**
 * OMP SDK Coding Client V2 for WebBrain.
 * Connects via localhost WebSocket to /webbrain/coding on the OMP SDK host.
 * Implements task-oriented coding handoff protocol v2, event normalization,
 * connection epochs, and explicit fallback telemetry.
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
  let capabilities = { read: true, write: true, command: true };
  let pendingRequests = new Map();
  let eventListeners = new Set();
  let reconnectTimer = null;
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

  function getStatus() {
    return {
      backend: 'omp-sdk-v2',
      connected,
      authenticated,
      bridgeUrl,
      root: rootPath,
      rootName,
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
          client: 'webbrain-extension',
          token: pairingToken || undefined,
        };

        pendingRequests.set(helloId, {
          resolve: (res) => {
            clearTimeout(timeout);
            authenticated = true;
            resolve({ ok: true, status: getStatus(), session: res });
          },
          reject: (err) => {
            clearTimeout(timeout);
            connected = false;
            authenticated = false;
            reject(err);
          },
        });

        ws.send(JSON.stringify(helloMsg));
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
        notifyListeners('disconnected', {});
      };
    });
  }

  function handleHostEvent(type, payload) {
    switch (type) {
      case 'coding.progress':
        lastProgressState = payload.state || 'running';
        taskStatus = payload.state === 'completed' ? 'completed' : payload.state === 'failed' ? 'failed' : 'running';
        notifyListeners('progress', payload);
        break;
      case 'coding.tool_activity':
        notifyListeners('tool_activity', payload);
        break;
      case 'coding.changed_files':
        notifyListeners('changed_files', payload);
        break;
      case 'coding.verification_requested':
        taskStatus = 'ready_for_browser_verification';
        notifyListeners('verification_requested', payload);
        break;
      case 'coding.completed':
        taskStatus = 'completed';
        activeTaskId = null;
        notifyListeners('completed', payload);
        break;
      case 'coding.failed':
        taskStatus = 'failed';
        activeTaskId = null;
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
    const res = await sendRequest('workspace.open', { workspacePath });
    currentWorkspaceId = res.workspaceId || 'ws-default';
    rootPath = res.rootPath || workspacePath;
    rootName = res.rootName || rootPath.split(/[/\\]/).pop() || 'workspace';
    if (res.capabilities) capabilities = res.capabilities;
    return res;
  }

  async function startTask(taskSpec) {
    taskStatus = 'starting';
    const res = await sendRequest('coding.start', {
      workspaceId: currentWorkspaceId,
      task: taskSpec,
    });
    activeTaskId = res.taskId;
    activeSessionId = res.sessionId;
    taskStatus = 'running';
    return res;
  }

  async function steerTask(message) {
    return await sendRequest('coding.steer', {
      taskId: activeTaskId,
      message,
    });
  }

  async function followUp(message) {
    return await sendRequest('coding.follow_up', {
      taskId: activeTaskId,
      message,
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

  async function submitVerification(result) {
    return await sendRequest('verification.result', {
      taskId: activeTaskId,
      result,
    });
  }

  async function closeSession() {
    try {
      if (activeTaskId) await abortTask();
      await sendRequest('session.close', { sessionId: activeSessionId });
    } catch {}
    if (ws) {
      try { ws.close(); } catch {}
      ws = null;
    }
    connected = false;
    authenticated = false;
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
