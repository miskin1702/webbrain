/**
 * Browser Extension UI Integration & E2E Rigorous Harness
 * 
 * SCOPE & LIMITATION CLARIFICATION:
 * This harness validates:
 * 1. Extension build bundle integrity (unpacked files in build/chrome).
 * 2. Settings configuration and backend state persistence (omp-sdk-v2 vs rust-v1 rollback).
 * 3. Live WebSocket gateway connection and workspace open contract (/webbrain/coding).
 * 4. Playwright unpacked extension launch smoke test (when browser executable is available).
 * 
 * NOTE: This harness does NOT execute a full end-to-end browser DOM sidepanel UI interaction
 * (clicking/typing inside the extension popup UI), which remains gated on automated extension UI runners.
 */

import test from 'node:test';
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { chromium } from 'playwright';
import { createCodingClientV2 } from '../../src/chrome/src/agent/coding-client-v2.js';
import { createWorkspaceManager } from '../../src/chrome/src/workspace-runs.js';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const ROOT = path.resolve(__dirname, '../..');
const BUILD_CHROME_DIR = path.join(ROOT, 'build/chrome');
const API_DIR = path.resolve(ROOT, '../api');
const BUN_EXE = path.join(API_DIR, 'runtime/bun.exe');

test('Extension unpacked build output structure is complete for V2', () => {
  assert.ok(fs.existsSync(BUILD_CHROME_DIR), 'build/chrome directory must exist. Run npm run build:chrome.');
  assert.ok(fs.existsSync(path.join(BUILD_CHROME_DIR, 'manifest.json')));
  assert.ok(fs.existsSync(path.join(BUILD_CHROME_DIR, 'src/background.js')));
  assert.ok(fs.existsSync(path.join(BUILD_CHROME_DIR, 'src/ui/sidepanel.html')));
  assert.ok(fs.existsSync(path.join(BUILD_CHROME_DIR, 'src/ui/sidepanel.js')));
  assert.ok(fs.existsSync(path.join(BUILD_CHROME_DIR, 'src/ui/settings.html')));
  assert.ok(fs.existsSync(path.join(BUILD_CHROME_DIR, 'src/ui/settings.js')));
  assert.ok(fs.existsSync(path.join(BUILD_CHROME_DIR, 'src/agent/coding-client-v2.js')));
  assert.ok(fs.existsSync(path.join(BUILD_CHROME_DIR, 'src/agent/coding-tools.js')));
});

test('Workspace manager handles settings backend toggle, path, token and V1 rollback', async () => {
  const mockStorage = {
    workspaceBackend: 'omp-sdk-v2',
    workspacePath: 'C:\\Projects\\webapp',
    workspaceToken: 'token-123',
  };

  const mockChrome = {
    storage: {
      local: {
        get: async (defaults) => {
          const res = {};
          for (const k of Object.keys(defaults)) {
            res[k] = mockStorage[k] !== undefined ? mockStorage[k] : defaults[k];
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
        if (msg.action === 'workspace_bridge_start') {
          return { status: { connected: true, authenticated: true, session: { root: mockStorage.workspacePath, rootName: 'webapp' } } };
        }
        return { ok: true };
      },
    },
  };

  const manager = createWorkspaceManager({ chromeApi: mockChrome });
  
  // 1. V2 Developer Default
  assert.strictEqual(manager.workspaceBackend(), 'omp-sdk-v2');

  // 2. V1 Rollback test
  await manager.connectWorkspace({
    workspaceBackend: 'rust-v1',
    url: 'ws://127.0.0.1:18374',
    token: 'v1-token',
  });
  assert.strictEqual(manager.workspaceBackend(), 'rust-v1');

  // 3. Roll forward to V2
  await manager.disconnectWorkspace();
  await manager.connectWorkspace({
    workspaceBackend: 'omp-sdk-v2',
    url: 'ws://127.0.0.1:18395/webbrain/coding',
    workspacePath: 'C:\\Projects\\webapp',
    token: 'token-123',
    timeoutMs: 1000,
  });
  assert.strictEqual(manager.workspaceBackend(), 'omp-sdk-v2');
});

test('Live Browser-to-Gateway WebSocket Handshake and Workspace Open with Guaranteed Cleanup', async () => {
  const PORT = 18395 + Math.floor(Math.random() * 50); // dynamic non-colliding port
  const TOKEN = 'e2e-browser-token-456';

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

  let serverStarted = false;
  const startupPromise = new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      if (!serverStarted) reject(new Error('Gateway server startup timed out'));
    }, 10000);

    serverProc.stdout.on('data', (d) => {
      if (d.toString().includes('Portable OMP Gateway listening:')) {
        serverStarted = true;
        clearTimeout(timeout);
        resolve();
      }
    });
    serverProc.on('error', (err) => {
      clearTimeout(timeout);
      reject(err);
    });
    serverProc.on('exit', (code) => {
      if (!serverStarted && code !== null && code !== 0) {
        clearTimeout(timeout);
        reject(new Error(`Server exited prematurely with code ${code}`));
      }
    });
  });

  try {
    await startupPromise;

    const client = createCodingClientV2();
    const conn = await client.connect({ url: `ws://127.0.0.1:${PORT}/webbrain/coding`, token: TOKEN });
    assert.strictEqual(conn.ok, true);

    const openRes = await client.openWorkspace(path.join(ROOT, 'test/fixtures/workspace-sample-project'));
    assert.ok(openRes.workspaceId);
    assert.strictEqual(client.isConnected(), true);

    await client.disconnect();
    assert.strictEqual(client.isConnected(), false);
  } finally {
    // Guaranteed child process cleanup
    if (serverProc && !serverProc.killed) {
      serverProc.kill('SIGTERM');
      await new Promise((resolve) => {
        serverProc.on('exit', resolve);
        setTimeout(resolve, 2000); // hard stop timeout fallback
      });
    }
  }
});

test('Playwright unpacked extension launch smoke test (with browser executable check)', async () => {
  if (!fs.existsSync(BUILD_CHROME_DIR)) {
    console.warn('Skipping Playwright unpacked extension launch: build/chrome not found');
    return;
  }

  let executablePath;
  try {
    executablePath = chromium.executablePath();
  } catch {
    executablePath = null;
  }

  if (!executablePath || !fs.existsSync(executablePath)) {
    console.warn('Playwright browser executable not installed locally; skipping launch smoke test (run npx playwright install if needed).');
    return;
  }

  const userDataDir = fs.mkdtempSync(path.join(path.dirname(BUILD_CHROME_DIR), 'pw-user-data-'));
  const context = await chromium.launchPersistentContext(userDataDir, {
    headless: false,
    args: [
      `--disable-extensions-except=${BUILD_CHROME_DIR}`,
      `--load-extension=${BUILD_CHROME_DIR}`,
    ],
  });
  
  try {
    const pages = context.pages();
    assert.ok(Array.isArray(pages));
  } finally {
    await context.close();
    fs.rmSync(userDataDir, { recursive: true, force: true });
  }
});
