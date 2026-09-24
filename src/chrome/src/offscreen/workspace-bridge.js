/**
 * Offscreen document — persistent WebSocket transport for WebBrain Local Workspace Bridge.
 *
 * Maintains a persistent, authenticated loopback WebSocket to the local Rust
 * daemon (workspace-bridge), handles message correlation, event forwarding,
 * and automatic reconnect with exponential backoff.
 */

(() => {
  const WORKSPACE_PROTOCOL_VERSION = 1;
  const DEFAULT_WORKSPACE_BRIDGE_URL = 'ws://127.0.0.1:18374';
  const DEFAULT_TIMEOUT_MS = 30000;

  let socket = null;
  let bridgeUrl = DEFAULT_WORKSPACE_BRIDGE_URL;
  let pairingToken = '';
  let enabled = false;
  let authenticated = false;
  let session = null;
  let reconnectTimer = null;
  let reconnectAttempt = 0;
  let lastError = '';
  let reqCounter = 0;

  // Correlation map: requestId -> { resolve, reject, timer, method }
  const pendingRequests = new Map();

  function normalizeWorkspaceUrl(value) {
    const raw = String(value || DEFAULT_WORKSPACE_BRIDGE_URL).trim();
    const url = new URL(raw.startsWith('ws://') || raw.startsWith('wss://') ? raw : `ws://${raw}`);
    const host = url.hostname.toLowerCase();
    if (url.protocol !== 'ws:' || !['127.0.0.1', 'localhost', '::1', '[::1]'].includes(host)) {
      throw new Error('Workspace bridge URL must use ws:// on localhost (127.0.0.1).');
    }
    return url.href;
  }

  function getStatus() {
    return {
      enabled,
      url: bridgeUrl,
      connected: socket?.readyState === WebSocket.OPEN,
      authenticated,
      readyState: socket ? socket.readyState : null,
      session,
      reconnectAttempt,
      lastError,
    };
  }

  function clearPendingRequests(error) {
    for (const [id, req] of pendingRequests) {
      clearTimeout(req.timer);
      req.reject(error || new Error('Workspace bridge connection closed'));
    }
    pendingRequests.clear();
  }

  function scheduleReconnect() {
    if (!enabled || reconnectTimer) return;
    const delay = Math.min(30000, 500 * Math.pow(2, reconnectAttempt++));
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      connect();
    }, delay);
  }

  function sendRpcCall(method, params = {}, timeoutMs = DEFAULT_TIMEOUT_MS) {
    return new Promise((resolve, reject) => {
      if (!socket || socket.readyState !== WebSocket.OPEN) {
        return reject(new Error('Workspace bridge is not connected.'));
      }
      if (!authenticated && method !== 'auth.handshake') {
        return reject(new Error('Workspace bridge is not authenticated.'));
      }

      const id = `wb_req_${Date.now()}_${++reqCounter}`;
      const timer = setTimeout(() => {
        pendingRequests.delete(id);
        const err = new Error(`Workspace RPC call "${method}" timed out after ${timeoutMs}ms.`);
        err.code = 'COMMAND_TIMEOUT';
        reject(err);
      }, timeoutMs);

      pendingRequests.set(id, { resolve, reject, timer, method });

      const envelope = {
        v: WORKSPACE_PROTOCOL_VERSION,
        id,
        method,
        params,
      };

      try {
        socket.send(JSON.stringify(envelope));
      } catch (err) {
        clearTimeout(timer);
        pendingRequests.delete(id);
        reject(err);
      }
    });
  }

  async function performHandshake(targetSocket) {
    const handshakeId = `wb_auth_${Date.now()}_${++reqCounter}`;
    const envelope = {
      v: WORKSPACE_PROTOCOL_VERSION,
      id: handshakeId,
      method: 'auth.handshake',
      params: {
        token: pairingToken || '',
        client: 'webbrain-extension',
        extensionId: (typeof chrome !== 'undefined' && chrome.runtime?.id) ? chrome.runtime.id : undefined,
        protocolVersion: WORKSPACE_PROTOCOL_VERSION,
      },
    };

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pendingRequests.delete(handshakeId);
        reject(new Error('Handshake with workspace bridge timed out after 10000ms.'));
      }, 10000);

      pendingRequests.set(handshakeId, {
        resolve: (result) => {
          clearTimeout(timer);
          resolve(result);
        },
        reject: (err) => {
          clearTimeout(timer);
          reject(err);
        },
        timer,
        method: 'auth.handshake',
      });

      try {
        targetSocket.send(JSON.stringify(envelope));
      } catch (e) {
        clearTimeout(timer);
        pendingRequests.delete(handshakeId);
        reject(e);
      }
    });
  }

  function connect() {
    if (!enabled || !bridgeUrl) return;
    if (socket && (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)) {
      return;
    }

    try {
      const nextSocket = new WebSocket(bridgeUrl);
      socket = nextSocket;

      nextSocket.addEventListener('open', async () => {
        if (socket !== nextSocket) return;
        lastError = '';

        try {
          const authResult = await performHandshake(nextSocket);
          if (socket !== nextSocket) return;
          authenticated = true;
          session = authResult;
          reconnectAttempt = 0;
          lastError = '';

          // Broadcast connected event to background
          try {
            chrome.runtime.sendMessage({
              target: 'background',
              action: 'workspace_connected',
              session: authResult,
            }).catch(() => {});
          } catch {}
        } catch (authError) {
          if (socket !== nextSocket) return;
          lastError = authError.message || String(authError);
          authenticated = false;
          session = null;
          try { nextSocket.close(); } catch {}
        }
      });

      nextSocket.addEventListener('message', (event) => {
        if (socket !== nextSocket) return;

        let msg;
        try {
          msg = JSON.parse(event.data);
        } catch (e) {
          lastError = `Malformed JSON frame: ${e.message}`;
          return;
        }

        // 1. Daemon push event (e.g. file.changed)
        if (msg.event) {
          try {
            chrome.runtime.sendMessage({
              target: 'background',
              action: 'workspace_event',
              event: msg.event,
              data: msg.data,
            }).catch(() => {});
          } catch {}
          return;
        }

        // 2. Response to a correlated request
        const id = msg.id;
        if (id && pendingRequests.has(id)) {
          const req = pendingRequests.get(id);
          clearTimeout(req.timer);
          pendingRequests.delete(id);

          if (msg.ok) {
            req.resolve(msg.result);
          } else {
            const err = new Error(msg.error?.message || 'Workspace bridge RPC error');
            err.code = msg.error?.code || 'INTERNAL_ERROR';
            err.details = msg.error;
            req.reject(err);
          }
        }
      });

      nextSocket.addEventListener('close', () => {
        if (socket !== nextSocket) return;
        socket = null;
        authenticated = false;
        session = null;
        clearPendingRequests(new Error('Workspace bridge connection closed'));

        try {
          chrome.runtime.sendMessage({
            target: 'background',
            action: 'workspace_disconnected',
          }).catch(() => {});
        } catch {}

        scheduleReconnect();
      });

      nextSocket.addEventListener('error', () => {
        if (socket !== nextSocket) return;
        lastError = 'WebSocket connection error';
      });
    } catch (e) {
      lastError = e.message || String(e);
      socket = null;
      authenticated = false;
      session = null;
      scheduleReconnect();
    }
  }

  // Chrome runtime message routing
  chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
    const action = msg.action || msg.type;

    if (action === 'workspace_bridge_start' || action === 'workspace-bridge-start') {
      try {
        const nextUrl = normalizeWorkspaceUrl(msg.url || bridgeUrl);
        const changed = bridgeUrl !== nextUrl || pairingToken !== (msg.token || '');
        bridgeUrl = nextUrl;
        pairingToken = String(msg.token || '').trim();
        enabled = true;

        if (changed && socket) {
          const prev = socket;
          socket = null;
          authenticated = false;
          session = null;
          clearPendingRequests(new Error('Reconnecting with updated workspace config'));
          try { prev.close(); } catch {}
        }

        connect();
        sendResponse({ ok: true, status: getStatus() });
      } catch (err) {
        lastError = err.message || String(err);
        sendResponse({ ok: false, error: lastError, status: getStatus() });
      }
      return false;
    }

    if (action === 'workspace_bridge_stop' || action === 'workspace-bridge-stop') {
      enabled = false;
      if (reconnectTimer) clearTimeout(reconnectTimer);
      reconnectTimer = null;
      reconnectAttempt = 0;
      authenticated = false;
      session = null;

      if (socket) {
        const prev = socket;
        socket = null;
        try { prev.close(); } catch {}
      }
      clearPendingRequests(new Error('Workspace bridge stopped by user'));
      sendResponse({ ok: true, status: getStatus() });
      return false;
    }

    if (action === 'workspace_bridge_status' || action === 'workspace-bridge-status') {
      sendResponse({ ok: true, status: getStatus() });
      return false;
    }

    if (action === 'workspace_bridge_call' || action === 'workspace-bridge-call') {
      const { method, params, timeoutMs } = msg;
      sendRpcCall(method, params, timeoutMs)
        .then((result) => {
          sendResponse({ ok: true, result });
        })
        .catch((error) => {
          sendResponse({
            ok: false,
            error: error.message || String(error),
            code: error.code || 'INTERNAL_ERROR',
            details: error.details,
          });
        });
      return true; // Keep message channel open for async response
    }

    return false;
  });
})();
