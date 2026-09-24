/**
 * Tests for WebBrain Local Workspace Bridge — Browser Extension Integration (Phases 3 & 4)
 *
 * Covers:
 * 1. Tool registry schemas, mode filtering, and capability gating
 * 2. Security classification and untrusted boundary integration
 * 3. Workspace manager lifecycle, tool execution, and revision tracking
 * 4. Chrome and Firefox parity
 */

import { strict as assert } from 'node:assert';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const ROOT = path.resolve(__dirname, '..', '..');
const toUrl = (rel) => pathToFileURL(path.join(ROOT, rel)).href;

// 1. Load Chrome and Firefox modules
const {
  WORKSPACE_ALL_TOOLS,
  WORKSPACE_READ_TOOLS,
  WORKSPACE_WRITE_TOOLS,
  WORKSPACE_COMMAND_TOOLS,
  WORKSPACE_TOOL_NAMES,
  SYSTEM_PROMPT_WORKSPACE,
} = await import(toUrl('src/chrome/src/agent/workspace-tools.js'));

const {
  WORKSPACE_ALL_TOOLS: WORKSPACE_ALL_TOOLS_FX,
  WORKSPACE_TOOL_NAMES: WORKSPACE_TOOL_NAMES_FX,
} = await import(toUrl('src/firefox/src/agent/workspace-tools.js'));

const { getToolsForMode: getToolsCh } = await import(toUrl('src/chrome/src/agent/tools.js'));
const { getToolsForMode: getToolsFx } = await import(toUrl('src/firefox/src/agent/tools.js'));

const {
  Capability: CapCh,
  capabilityFor: capForCh,
  hostForCapability: hostForCh,
  UNTRUSTED_CONTENT_TOOLS: UCT_CH,
} = await import(toUrl('src/chrome/src/agent/permission-gate.js'));

const {
  Capability: CapFx,
  capabilityFor: capForFx,
  hostForCapability: hostForFx,
  UNTRUSTED_CONTENT_TOOLS: UCT_FX,
} = await import(toUrl('src/firefox/src/agent/permission-gate.js'));

const { createWorkspaceManager } = await import(toUrl('src/chrome/src/workspace-runs.js'));

let passed = 0;
let total = 0;
const asyncTasks = [];

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

function asyncTest(name, fn) {
  total++;
  const p = (async () => {
    try {
      await fn();
      passed++;
      console.log(`  ✓ ${name}`);
    } catch (e) {
      console.error(`  ✗ ${name}`);
      console.error(`      ${e.message}`);
      throw e;
    }
  })();
  asyncTasks.push(p);
}

console.log('\n--- Workspace Tool Registry & Parity Tests ---');

test('all 8 workspace tools have valid OpenAI function schemas', () => {
  const expectedNames = [
    'workspace_status',
    'workspace_search_code',
    'workspace_read_file',
    'workspace_read_range',
    'workspace_apply_patch',
    'workspace_create_file',
    'workspace_git_diff',
    'workspace_run_command',
  ];

  assert.equal(WORKSPACE_ALL_TOOLS.length, 8);
  for (const tool of WORKSPACE_ALL_TOOLS) {
    assert.equal(tool.type, 'function');
    assert.ok(tool.function.name);
    assert.ok(expectedNames.includes(tool.function.name));
    assert.ok(tool.function.description.length > 20);
    assert.equal(tool.function.parameters.type, 'object');
    assert.ok(Array.isArray(tool.function.parameters.required));
  }
});

test('workspace search tool requires query and declares options', () => {
  const search = WORKSPACE_ALL_TOOLS.find(t => t.function.name === 'workspace_search_code');
  assert.ok(search);
  assert.deepEqual(search.function.parameters.required, ['query']);
  assert.ok(search.function.parameters.properties.query);
  assert.ok(search.function.parameters.properties.limit);
  assert.ok(search.function.parameters.properties.is_regex);
  assert.ok(search.function.parameters.properties.case_sensitive);
  assert.ok(search.function.parameters.properties.include);
  assert.ok(search.function.parameters.properties.exclude);
});

test('workspace read tools require path and bounds', () => {
  const readFile = WORKSPACE_ALL_TOOLS.find(t => t.function.name === 'workspace_read_file');
  assert.ok(readFile);
  assert.deepEqual(readFile.function.parameters.required, ['path']);

  const readRange = WORKSPACE_ALL_TOOLS.find(t => t.function.name === 'workspace_read_range');
  assert.ok(readRange);
  assert.deepEqual(readRange.function.parameters.required, ['path', 'start_line', 'end_line']);
});

test('workspace apply patch tool requires path and expected_revision', () => {
  const patch = WORKSPACE_ALL_TOOLS.find(t => t.function.name === 'workspace_apply_patch');
  assert.ok(patch);
  assert.deepEqual(patch.function.parameters.required, ['path', 'expected_revision']);
  assert.ok(patch.function.parameters.properties.patch);
  assert.ok(patch.function.parameters.properties.old_text);
  assert.ok(patch.function.parameters.properties.new_text);
});

test('workspace create file tool requires path and content and declares overwrite', () => {
  const createFile = WORKSPACE_ALL_TOOLS.find(t => t.function.name === 'workspace_create_file');
  assert.ok(createFile);
  assert.deepEqual(createFile.function.parameters.required, ['path', 'content']);
  assert.ok(createFile.function.parameters.properties.path);
  assert.ok(createFile.function.parameters.properties.content);
  assert.ok(createFile.function.parameters.properties.overwrite);
});

test('workspace apply patch tool mentions creating new files', () => {
  const patch = WORKSPACE_ALL_TOOLS.find(t => t.function.name === 'workspace_apply_patch');
  assert.ok(patch);
  assert.match(patch.function.description, /create a new file/i);
});

test('Chrome and Firefox workspace tool definitions stay in parity', () => {
  assert.equal(WORKSPACE_ALL_TOOLS.length, WORKSPACE_ALL_TOOLS_FX.length);
  for (let i = 0; i < WORKSPACE_ALL_TOOLS.length; i++) {
    assert.equal(WORKSPACE_ALL_TOOLS[i].function.name, WORKSPACE_ALL_TOOLS_FX[i].function.name);
    assert.deepEqual(
      WORKSPACE_ALL_TOOLS[i].function.parameters.required,
      WORKSPACE_ALL_TOOLS_FX[i].function.parameters.required,
    );
  }
});

console.log('\n--- Dynamic Tool Exposure (getToolsForMode) Tests ---');

for (const [label, getTools] of [['chrome', getToolsCh], ['firefox', getToolsFx]]) {
  test(`[${label}] disconnected workspace NEVER includes workspace tools in any mode`, () => {
    for (const mode of ['ask', 'act', 'dev']) {
      for (const tier of ['compact', 'mid', 'full']) {
        const tools = getTools(mode, { tier });
        const wsTools = tools.filter(t => t.function.name.startsWith('workspace_'));
        assert.equal(wsTools.length, 0, `${label}/${mode}/${tier} leaked workspace tools when disconnected`);
      }
    }
  });

  test(`[${label}] Ask mode with workspace connected exposes ONLY read tools`, () => {
    const tools = getTools('ask', { workspaceConnected: true });
    const wsNames = tools.filter(t => t.function.name.startsWith('workspace_')).map(t => t.function.name);
    assert.deepEqual(wsNames.sort(), [
      'workspace_read_file',
      'workspace_read_range',
      'workspace_search_code',
      'workspace_status',
    ].sort());
  });

  test(`[${label}] Act mode with workspace connected exposes read + git_diff by default`, () => {
    const tools = getTools('act', { workspaceConnected: true });
    const wsNames = tools.filter(t => t.function.name.startsWith('workspace_')).map(t => t.function.name);
    assert.deepEqual(wsNames.sort(), [
      'workspace_git_diff',
      'workspace_read_file',
      'workspace_read_range',
      'workspace_search_code',
      'workspace_status',
    ].sort());
  });

  test(`[${label}] Act mode with workspaceCanWrite exposes workspace_apply_patch and workspace_create_file`, () => {
    const tools = getTools('act', { workspaceConnected: true, workspaceCanWrite: true });
    const wsNames = tools.filter(t => t.function.name.startsWith('workspace_')).map(t => t.function.name);
    assert.equal(wsNames.includes('workspace_apply_patch'), true);
    assert.equal(wsNames.includes('workspace_create_file'), true);
    assert.equal(wsNames.includes('workspace_run_command'), false);
  });
  test(`[${label}] Act mode with workspaceCanCommand exposes workspace_run_command`, () => {
    const tools = getTools('act', { workspaceConnected: true, workspaceCanCommand: true });
    const wsNames = tools.filter(t => t.function.name.startsWith('workspace_')).map(t => t.function.name);
    assert.equal(wsNames.includes('workspace_run_command'), true);
    assert.equal(wsNames.includes('workspace_apply_patch'), false);
  });

  test(`[${label}] Full capabilities expose all 8 workspace tools`, () => {
    const tools = getTools('act', {
      workspaceConnected: true,
      workspaceCanWrite: true,
      workspaceCanCommand: true,
    });
    const wsNames = tools.filter(t => t.function.name.startsWith('workspace_')).map(t => t.function.name);
    assert.equal(wsNames.length, 8);
  });
}

console.log('\n--- Security & Permission Integration Tests ---');

for (const [label, Cap, capFor, hostFor, uct] of [
  ['chrome', CapCh, capForCh, hostForCh, UCT_CH],
  ['firefox', CapFx, capForFx, hostForFx, UCT_FX],
]) {
  test(`[${label}] Capability definitions include workspace_write and workspace_command`, () => {
    assert.equal(Cap.WORKSPACE_WRITE, 'workspace_write');
    assert.equal(Cap.WORKSPACE_COMMAND, 'workspace_command');
  });

  test(`[${label}] workspace_apply_patch and workspace_create_file map to WORKSPACE_WRITE`, () => {
    assert.equal(capFor('workspace_apply_patch', {}), Cap.WORKSPACE_WRITE);
    assert.equal(capFor('workspace_create_file', {}), Cap.WORKSPACE_WRITE);
  });

  test(`[${label}] workspace_run_command maps to WORKSPACE_COMMAND`, () => {
    assert.equal(capFor('workspace_run_command', {}), Cap.WORKSPACE_COMMAND);
  });

  test(`[${label}] read-only workspace tools are ungated (capability === null)`, () => {
    for (const name of ['workspace_status', 'workspace_search_code', 'workspace_read_file', 'workspace_read_range', 'workspace_git_diff']) {
      assert.equal(capFor(name, {}), null, `${label}: ${name} should be ungated`);
    }
  });

  test(`[${label}] all 8 workspace tools are in UNTRUSTED_CONTENT_TOOLS`, () => {
    for (const tool of WORKSPACE_ALL_TOOLS) {
      assert.equal(uct.has(tool.function.name), true, `${label}: ${tool.function.name} must be in UNTRUSTED_CONTENT_TOOLS`);
    }
  });

  test(`[${label}] hostForCapability returns 'workspace' for workspace capabilities`, () => {
    assert.equal(hostFor(Cap.WORKSPACE_WRITE, {}, 'https://example.com'), 'workspace');
    assert.equal(hostFor(Cap.WORKSPACE_COMMAND, {}, 'https://example.com'), 'workspace');
  });
}

console.log('\n--- Workspace Manager (workspace-runs.js) Tests ---');

asyncTest('createWorkspaceManager initializes with clean disconnected status', async () => {
  const storageData = {};
  const mockChrome = {
    storage: {
      local: {
        get: async (defaults) => ({ ...defaults, ...storageData }),
        set: async (values) => { Object.assign(storageData, values); },
      },
    },
    runtime: {
      sendMessage: async (msg) => {
        if (msg.action === 'workspace_bridge_status') {
          return { status: { connected: false, authenticated: false } };
        }
        return { ok: true };
      },
    },
  };

  const mgr = createWorkspaceManager({ chromeApi: mockChrome, ensureOffscreen: async () => {} });
  const status = await mgr.getWorkspaceStatus();
  assert.equal(status.connected, false);
  assert.equal(status.authenticated, false);
  assert.equal(mgr.isConnected(), false);
  assert.equal(mgr.canWrite(), false);
  assert.equal(mgr.canCommand(), false);
});

asyncTest('executeWorkspaceTool fails gracefully when disconnected', async () => {
  const mockChrome = {
    storage: {
      local: {
        get: async (defaults) => defaults,
        set: async () => {},
      },
    },
    runtime: {
      sendMessage: async () => ({ status: { connected: false } }),
    },
  };

  const mgr = createWorkspaceManager({ chromeApi: mockChrome, ensureOffscreen: async () => {} });
  const res = await mgr.executeWorkspaceTool('workspace_search_code', { query: 'test' });
  assert.equal(res.success, false);
  assert.match(res.error, /not connected/i);
});

asyncTest('executeWorkspaceTool enforces write and command capabilities', async () => {
  let offscreenCall = null;
  const mockChrome = {
    storage: {
      local: {
        get: async (defaults) => defaults,
        set: async () => {},
      },
    },
    runtime: {
      sendMessage: async (msg) => {
        if (msg.action === 'workspace_bridge_start') {
          return {
            status: {
              connected: true,
              authenticated: true,
              session: { sessionId: 's1', root: '/test', rootName: 'test', capabilities: ['read', 'write'] },
            },
          };
        }
        if (msg.action === 'workspace_bridge_status') {
          return {
            status: {
              connected: true,
              authenticated: true,
              session: { sessionId: 's1', root: '/test', rootName: 'test' },
            },
          };
        }
        if (msg.action === 'workspace_bridge_call') {
          offscreenCall = msg;
          return { ok: true, result: { diff: 'mock diff' } };
        }
        return { ok: true };
      },
    },
  };

  const mgr = createWorkspaceManager({ chromeApi: mockChrome, ensureOffscreen: async () => {} });

  // Connect with write=false, command=false
  await mgr.connectWorkspace({ url: 'ws://127.0.0.1:18374', allowWrite: false, allowCommand: false });
  assert.equal(mgr.isConnected(), true);
  assert.equal(mgr.canWrite(), false);
  assert.equal(mgr.canCommand(), false);

  // Write should be rejected
  const patchRes = await mgr.executeWorkspaceTool('workspace_apply_patch', { path: 'a.js', expected_revision: 1, patch: '...' });
  assert.equal(patchRes.success, false);
  assert.match(patchRes.error, /write capability is not authorized/i);

  const createRes = await mgr.executeWorkspaceTool('workspace_create_file', { path: 'new.md', content: 'hello' });
  assert.equal(createRes.success, false);
  assert.match(createRes.error, /write capability is not authorized/i);

  // Command should be rejected
  const cmdRes = await mgr.executeWorkspaceTool('workspace_run_command', { command: 'cargo test' });
  assert.equal(cmdRes.success, false);
  assert.match(cmdRes.error, /command execution is not authorized/i);

  // Read / diff should succeed
  const diffRes = await mgr.executeWorkspaceTool('workspace_git_diff', {});
  assert.equal(diffRes.success, true);
  assert.equal(offscreenCall.method, 'workspace.git_diff');
});

asyncTest('file revision cache updates on read, patch, and file.changed events', async () => {
  const mockChrome = {
    storage: {
      local: {
        get: async (defaults) => defaults,
        set: async () => {},
      },
    },
    runtime: {
      sendMessage: async (msg) => {
        if (msg.action === 'workspace_bridge_start') {
          return { status: { connected: true, authenticated: true } };
        }
        if (msg.action === 'workspace_bridge_call') {
          if (msg.method === 'workspace.read_range') {
            return { ok: true, result: { path: 'src/main.rs', revision: 5, hash: 'h5' } };
          }
          if (msg.method === 'workspace.apply_patch') {
            return { ok: true, result: { path: 'src/main.rs', oldRevision: 5, newRevision: 6, newHash: 'h6' } };
          }
          if (msg.method === 'workspace.create_file') {
            return { ok: true, result: { path: 'src/new.rs', revision: 1, hash: 'h1' } };
          }
        }
        return { ok: true };
      },
    },
  };

  const mgr = createWorkspaceManager({ chromeApi: mockChrome, ensureOffscreen: async () => {} });
  await mgr.connectWorkspace({ allowWrite: true });

  // 1. Read updates cache
  await mgr.executeWorkspaceTool('workspace_read_range', { path: 'src/main.rs', start_line: 1, end_line: 10 });
  assert.deepEqual(mgr.fileRevisionCache.get('src/main.rs')?.revision, 5);

  // 2. Patch updates cache
  await mgr.executeWorkspaceTool('workspace_apply_patch', { path: 'src/main.rs', expected_revision: 5, patch: '...' });
  assert.deepEqual(mgr.fileRevisionCache.get('src/main.rs')?.revision, 6);

  // 3. External file.changed event updates cache
  mgr.handleWorkspaceEvent('file.changed', { path: 'src/main.rs', revision: 7, hash: 'h7' });
  assert.deepEqual(mgr.fileRevisionCache.get('src/main.rs')?.revision, 7);
  // 4. Create file updates cache
  await mgr.executeWorkspaceTool('workspace_create_file', { path: 'src/new.rs', content: 'fn main() {}' });
  assert.deepEqual(mgr.fileRevisionCache.get('src/new.rs')?.revision, 1);
});
console.log('\n--- Prompt Guidance Tests ---');

test('SYSTEM_PROMPT_WORKSPACE covers all required coding loop steps', () => {
  const prompt = SYSTEM_PROMPT_WORKSPACE;
  assert.match(prompt, /workspace_status/);
  assert.match(prompt, /workspace_search_code/);
  assert.match(prompt, /workspace_read_range/);
  assert.match(prompt, /expected_revision/);
  assert.match(prompt, /workspace_apply_patch/);
  assert.match(prompt, /workspace_git_diff/);
  assert.match(prompt, /workspace_run_command/);
  assert.match(prompt, /untrusted data/i);
});
await Promise.all(asyncTasks);

console.log(`\nAll ${total} tests passed! (${passed}/${total})`);
