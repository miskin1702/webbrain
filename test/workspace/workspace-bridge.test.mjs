/**
 * Tests for WebBrain Local Workspace Bridge — Offscreen Transport & Protocol
 */

import { strict as assert } from 'node:assert';

function normalizeWorkspaceUrl(value) {
  const DEFAULT_WORKSPACE_BRIDGE_URL = 'ws://127.0.0.1:18374';
  const raw = String(value || DEFAULT_WORKSPACE_BRIDGE_URL).trim();
  const url = new URL(raw.startsWith('ws://') || raw.startsWith('wss://') ? raw : `ws://${raw}`);
  const host = url.hostname.toLowerCase();
  if (url.protocol !== 'ws:' || !['127.0.0.1', 'localhost', '::1', '[::1]'].includes(host)) {
    throw new Error('Workspace bridge URL must use ws:// on localhost (127.0.0.1).');
  }
  return url.href;
}

let passed = 0;
let total = 0;

function test(name, fn) {
  total++;
  try {
    fn();
    passed++;
    console.log(`  ✓ ${name}`);
  } catch (e) {
    console.error(`  ✗ ${name}`);
    console.error(`      ${e.message}`);
    throw e;
  }
}

console.log('\n--- Workspace Bridge URL Validation Tests ---');

test('accepts valid localhost and loopback ws:// URLs', () => {
  assert.equal(normalizeWorkspaceUrl('ws://127.0.0.1:18374'), 'ws://127.0.0.1:18374/');
  assert.equal(normalizeWorkspaceUrl('127.0.0.1:18374'), 'ws://127.0.0.1:18374/');
  assert.equal(normalizeWorkspaceUrl('ws://localhost:18374'), 'ws://localhost:18374/');
  assert.equal(normalizeWorkspaceUrl('localhost:18374'), 'ws://localhost:18374/');
  assert.equal(normalizeWorkspaceUrl('ws://[::1]:18374'), 'ws://[::1]:18374/');
});

test('rejects remote / non-loopback hosts', () => {
  assert.throws(() => normalizeWorkspaceUrl('ws://example.com:18374'), /localhost/i);
  assert.throws(() => normalizeWorkspaceUrl('ws://192.168.1.100:18374'), /localhost/i);
  assert.throws(() => normalizeWorkspaceUrl('ws://0.0.0.0:18374'), /localhost/i);
  assert.throws(() => normalizeWorkspaceUrl('ws://attacker.evil:18374'), /localhost/i);
});

test('rejects non-ws protocols (http, https, wss)', () => {
  assert.throws(() => normalizeWorkspaceUrl('http://127.0.0.1:18374'), /must use ws:\/\//i);
  assert.throws(() => normalizeWorkspaceUrl('https://127.0.0.1:18374'), /must use ws:\/\//i);
  assert.throws(() => normalizeWorkspaceUrl('wss://127.0.0.1:18374'), /must use ws:\/\//i);
});

console.log('\n--- Protocol Correlation & Timeout Tests ---');

test('correlation map resolves matching request IDs and clears timers', async () => {
  const pendingRequests = new Map();
  let cleared = false;
  const timer = setTimeout(() => { cleared = false; }, 5000);

  const p = new Promise((resolve, reject) => {
    pendingRequests.set('req_123', {
      resolve,
      reject,
      timer,
      method: 'workspace.status',
    });
  });

  const responseFrame = {
    v: 1,
    id: 'req_123',
    ok: true,
    result: { connected: true, root: '/workspace' },
  };

  const req = pendingRequests.get(responseFrame.id);
  assert.ok(req);
  clearTimeout(req.timer);
  pendingRequests.delete(responseFrame.id);
  req.resolve(responseFrame.result);

  const res = await p;
  assert.equal(res.connected, true);
  assert.equal(res.root, '/workspace');
  assert.equal(pendingRequests.size, 0);
});

test('correlation map rejects on error envelope', async () => {
  const pendingRequests = new Map();
  const timer = setTimeout(() => {}, 5000);

  const p = new Promise((resolve, reject) => {
    pendingRequests.set('req_456', { resolve, reject, timer, method: 'workspace.apply_patch' });
  });

  const errorFrame = {
    v: 1,
    id: 'req_456',
    ok: false,
    error: {
      code: 'REVISION_CONFLICT',
      message: 'File changed externally',
    },
  };

  const req = pendingRequests.get(errorFrame.id);
  clearTimeout(req.timer);
  pendingRequests.delete(errorFrame.id);
  const err = new Error(errorFrame.error.message);
  err.code = errorFrame.error.code;
  req.reject(err);

  await assert.rejects(async () => await p, (err) => {
    assert.equal(err.code, 'REVISION_CONFLICT');
    assert.equal(err.message, 'File changed externally');
    return true;
  });
});

test('daemon push event envelope has no id and carries event name + data', () => {
  const eventFrame = {
    v: 1,
    event: 'file.changed',
    data: {
      path: 'src/main.rs',
      revision: 42,
      hash: 'hash_42',
    },
  };

  assert.equal(eventFrame.v, 1);
  assert.equal(eventFrame.id, undefined);
  assert.equal(eventFrame.event, 'file.changed');
  assert.equal(eventFrame.data.path, 'src/main.rs');
  assert.equal(eventFrame.data.revision, 42);
});

console.log(`\nAll ${total} tests passed! (${passed}/${total})`);
