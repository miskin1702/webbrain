/**
 * Unit and integration tests for OMP SDK V2 coding client, tools,
 * workspace manager dual-backend lifecycle, V2->V1->V2 rollback,
 * and live contract verification against the actual OMP Gateway server.
 */

import test from 'node:test';
import assert from 'node:assert';
import { spawn } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createCodingClientV2 } from '../../src/chrome/src/agent/coding-client-v2.js';
import { CODING_TOOLS, CODING_TOOL_NAMES, SYSTEM_PROMPT_OMP_CODING_V2 } from '../../src/chrome/src/agent/coding-tools.js';
import { getToolsForMode } from '../../src/chrome/src/agent/tools.js';
import { WORKSPACE_TOOL_NAMES } from '../../src/chrome/src/agent/workspace-tools.js';
import { createWorkspaceManager } from '../../src/chrome/src/workspace-runs.js';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const API_DIR = path.resolve(__dirname, '../../../api');
const BUN_EXE = path.join(API_DIR, 'runtime/bun.exe');

test('OMP SDK V2 coding client initializes with default status', () => {
  const client = createCodingClientV2();
  const status = client.getStatus();
  assert.strictEqual(status.backend, 'omp-sdk-v2');
  assert.strictEqual(status.connected, false);
  assert.strictEqual(status.authenticated, false);
  assert.strictEqual(status.fallbackUsed, false);
});

test('Coding tools definitions are complete and valid', () => {
  assert.strictEqual(CODING_TOOLS.length, 4);
  assert.ok(CODING_TOOL_NAMES.has('coding_delegate'));
  assert.ok(CODING_TOOL_NAMES.has('coding_steer'));
  assert.ok(CODING_TOOL_NAMES.has('coding_status'));
  assert.ok(CODING_TOOL_NAMES.has('coding_abort'));
  assert.ok(SYSTEM_PROMPT_OMP_CODING_V2.includes('OMP SDK CODING WORKER (V2)'));
});

test('getToolsForMode hides low-level workspace tools when workspaceBackend is omp-sdk-v2', () => {
  const toolsV2 = getToolsForMode('act', {
    workspaceConnected: true,
    workspaceBackend: 'omp-sdk-v2',
  });

  const toolNames = new Set(toolsV2.map(t => t.function.name));

  // Verify high-level coding tools are present
  assert.ok(toolNames.has('coding_delegate'), 'coding_delegate should be exposed');
  assert.ok(toolNames.has('coding_steer'), 'coding_steer should be exposed');

  // Verify low-level workspace tools are hidden
  for (const lowLevelName of WORKSPACE_TOOL_NAMES) {
    assert.strictEqual(
      toolNames.has(lowLevelName),
      false,
      `Low-level workspace tool ${lowLevelName} should be hidden in V2 mode`
    );
  }
});

test('getToolsForMode retains low-level workspace tools when workspaceBackend is rust-v1 (fallback mode)', () => {
  const toolsV1 = getToolsForMode('act', {
    workspaceConnected: true,
    workspaceCanWrite: true,
    workspaceBackend: 'rust-v1',
  });

  const toolNames = new Set(toolsV1.map(t => t.function.name));

  // Verify low-level workspace tools are present in fallback mode
  assert.ok(toolNames.has('workspace_search_code'), 'workspace_search_code should be present in V1 fallback');
  assert.ok(toolNames.has('workspace_read_range'), 'workspace_read_range should be present in V1 fallback');
  assert.ok(toolNames.has('workspace_apply_patch'), 'workspace_apply_patch should be present in V1 fallback');

  // Verify high-level coding tools are not exposed in V1 fallback mode
  assert.strictEqual(toolNames.has('coding_delegate'), false);
});

test('workspaceManager initializes with omp-sdk-v2 developer default and exposes status without bridge error', async () => {
  const mockStorage = {};
  const mockChrome = {
    storage: {
      local: {
        get: async (defaults) => {
          const res = {};
          for (const k of Object.keys(defaults)) {
            res[k] = mockStorage[k] || defaults[k];
          }
          return res;
        },
        set: async (items) => {
          Object.assign(mockStorage, items);
        },
      },
    },
    runtime: {
      sendMessage: async () => ({ ok: true }),
    },
  };

  const manager = createWorkspaceManager({ chromeApi: mockChrome });
  assert.strictEqual(manager.workspaceBackend(), 'omp-sdk-v2');

  // workspace_status immediately answers directly without falling into bridge call
  const statusRes = await manager.executeWorkspaceTool('workspace_status');
  assert.strictEqual(statusRes.success, true);
  assert.strictEqual(statusRes.result.backend, 'omp-sdk-v2');
  assert.strictEqual(statusRes.result.connected, false);
});

test('V2 -> V1 -> V2 rollback sequence operates reliably in workspaceManager', async () => {
  const mockStorage = {};
  let sentMessages = [];
  const mockChrome = {
    storage: {
      local: {
        get: async (defaults) => {
          const res = {};
          for (const k of Object.keys(defaults)) {
            res[k] = mockStorage[k] || defaults[k];
          }
          return res;
        },
        set: async (items) => {
          Object.assign(mockStorage, items);
        },
      },
    },
    runtime: {
      sendMessage: async (msg) => {
        sentMessages.push(msg);
        if (msg.action === 'workspace_bridge_start') {
          return { status: { connected: true, authenticated: true, session: { root: '/projects/v1', rootName: 'v1' } } };
        }
        if (msg.action === 'workspace_bridge_stop') {
          return { ok: true };
        }
        return { ok: true };
      },
    },
  };

  const manager = createWorkspaceManager({ chromeApi: mockChrome });

  // 1. Initial V2 state
  assert.strictEqual(manager.workspaceBackend(), 'omp-sdk-v2');

  // 2. Rollback to V1
  const v1Status = await manager.connectWorkspace({
    workspaceBackend: 'rust-v1',
    url: 'ws://127.0.0.1:18374',
    token: 'test-v1-token',
  });

  assert.strictEqual(manager.workspaceBackend(), 'rust-v1');
  assert.strictEqual(v1Status.backend, 'rust-v1');
  assert.strictEqual(v1Status.connected, true);
  assert.strictEqual(v1Status.rootName, 'v1');

  // In V1, coding_delegate should be rejected with fallback error
  const rejectCoding = await manager.executeWorkspaceTool('coding_delegate', { summary: 'test' });
  assert.strictEqual(rejectCoding.success, false);
  assert.ok(rejectCoding.error.includes('requires OMP SDK V2 mode'));

  // 3. Roll back to V2
  await manager.disconnectWorkspace();
  const v2Status = await manager.connectWorkspace({
    workspaceBackend: 'omp-sdk-v2',
    url: 'ws://127.0.0.1:18374/webbrain/coding',
    workspacePath: 'C:\\Projects\\test',
    timeoutMs: 500,
  });

  assert.strictEqual(manager.workspaceBackend(), 'omp-sdk-v2');
  assert.strictEqual(v2Status.backend, 'omp-sdk-v2');
});

test('live contract verification: codingClientV2 connects to actual Portable OMP Gateway server', async () => {
  const PORT = 18388;
  const TOKEN = 'contract-test-token-777';

  // Spawn actual Portable OMP Gateway server from api repo
  const serverProc = spawn(
    BUN_EXE,
    ['run', 'src/cli.ts', 'server'],
    {
      cwd: API_DIR,
      env: {
        ...process.env,
        PORT: String(PORT),
        CONTROL_PORT: String(PORT + 1),
        OMP_GATEWAY_PORT: String(PORT),
        OMP_GATEWAY_TOKEN: TOKEN,
        GATEWAY_API_TOKEN: TOKEN,
        WEBBRAIN_CODING_ENABLED: 'true',
        WEBBRAIN_CODING_TOKEN: TOKEN,
        WEBBRAIN_CODING_PATH: '/webbrain/coding',
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    }
  );

  await new Promise((resolve, reject) => {
    serverProc.stdout.on('data', (d) => {
      if (d.toString().includes('Portable OMP Gateway listening:')) {
        resolve();
      }
    });
    serverProc.stderr.on('data', (d) => {
      // console.error('[server stderr]', d.toString());
    });
    serverProc.on('error', reject);
    serverProc.on('exit', (code) => {
      if (code !== null && code !== 0) reject(new Error(`Server exited with code ${code}`));
    });
  });

  const url = `ws://127.0.0.1:${PORT}/webbrain/coding`;

  try {
    const client = createCodingClientV2();

    // 1. Connect & Handshake
    const connectRes = await client.connect({ url, token: TOKEN });
    assert.strictEqual(connectRes.ok, true);
    assert.strictEqual(client.isConnected(), true);
    assert.strictEqual(connectRes.session?.server?.name, 'portable-omp-gateway');

    // 2. Open Workspace with absolute root path
    const openRes = await client.openWorkspace(API_DIR);
    assert.ok(openRes.workspaceId);
    assert.strictEqual(client.getStatus().connected, true);
    assert.strictEqual(client.getStatus().authenticated, true);
    // 3. Close Session
    await client.disconnect();
    assert.strictEqual(client.isConnected(), false);
  } finally {
    serverProc.kill();
  }
});
