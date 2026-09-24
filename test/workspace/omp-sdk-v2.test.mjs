/**
 * Unit and integration tests for OMP SDK V2 coding client, tools, and feature flag separation.
 */

import test from 'node:test';
import assert from 'node:assert';
import { createCodingClientV2 } from '../../src/chrome/src/agent/coding-client-v2.js';
import { CODING_TOOLS, CODING_TOOL_NAMES, SYSTEM_PROMPT_OMP_CODING_V2 } from '../../src/chrome/src/agent/coding-tools.js';
import { getToolsForMode } from '../../src/chrome/src/agent/tools.js';
import { WORKSPACE_TOOL_NAMES } from '../../src/chrome/src/agent/workspace-tools.js';

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
