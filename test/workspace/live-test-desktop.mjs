/**
 * Live test script for WebBrain Workspace Daemon against Desktop test directory.
 */

import { strict as assert } from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';

const PORT = 18374;
const URL = `ws://127.0.0.1:${PORT}`;
const TOKEN = 'test';
const TEST_DIR = 'C:\\Users\\miski\\Desktop\\test';

class BridgeClient {
  constructor(url) {
    this.url = url;
    this.ws = null;
    this.pending = new Map();
    this.nextId = 1;
    this.events = [];
    this.eventListeners = [];
  }

  async connect() {
    return new Promise((resolve, reject) => {
      this.ws = new WebSocket(this.url);

      this.ws.onopen = () => resolve();
      this.ws.onerror = (err) => reject(new Error(`WebSocket error: ${err.message || err}`));

      this.ws.onmessage = (event) => {
        try {
          const msg = JSON.parse(event.data);
          if (msg.event) {
            this.events.push(msg);
            for (const listener of this.eventListeners) {
              listener(msg);
            }
          } else if (msg.id) {
            const resolver = this.pending.get(msg.id);
            if (resolver) {
              this.pending.delete(msg.id);
              resolver(msg);
            }
          }
        } catch (e) {
          console.error('[client] Failed to parse message:', e);
        }
      };

      this.ws.onclose = () => {
        for (const resolver of this.pending.values()) {
          resolver({ ok: false, error: { code: 'DISCONNECTED', message: 'Socket closed' } });
        }
        this.pending.clear();
      };
    });
  }

  async request(method, params = null) {
    const id = `req_${this.nextId++}`;
    const payload = {
      v: 1,
      id,
      method,
      params,
    };

    return new Promise((resolve, reject) => {
      this.pending.set(id, (resp) => {
        resolve(resp);
      });
      this.ws.send(JSON.stringify(payload));
    });
  }

  close() {
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
  }
}

async function run() {
  // Clean up any test files from prior runs so tests start from a clean state
  const selamDiskPath = path.join(TEST_DIR, 'selam.md');
  const patchDiskPath = path.join(TEST_DIR, 'selam_patch.md');
  if (fs.existsSync(selamDiskPath)) fs.unlinkSync(selamDiskPath);
  if (fs.existsSync(patchDiskPath)) fs.unlinkSync(patchDiskPath);

  console.log(`Connecting to workspace daemon at ${URL}...`);
  const client = new BridgeClient(URL);
  await client.connect();
  console.log('✓ Connected to WebSocket.');

  // 1. auth.handshake
  console.log('Executing auth.handshake...');
  const handshakeResp = await client.request('auth.handshake', {
    token: TOKEN,
    client: 'webbrain-desktop-live-test',
    protocolVersion: 1,
  });
  console.log('Handshake response:', JSON.stringify(handshakeResp));
  assert.equal(handshakeResp.ok, true, `Handshake failed: ${JSON.stringify(handshakeResp.error)}`);
  assert.equal(handshakeResp.result.protocolVersion, 1);
  assert.ok(handshakeResp.result.capabilities.includes('read'));
  assert.ok(handshakeResp.result.capabilities.includes('write'));
  console.log('✓ auth.handshake succeeded.');

  // 2. workspace.status
  console.log('Checking workspace.status...');
  const statusResp = await client.request('workspace.status');
  console.log('Status response:', JSON.stringify(statusResp));
  assert.equal(statusResp.ok, true, `Status check failed: ${JSON.stringify(statusResp.error)}`);
  assert.equal(statusResp.result.connected, true);
  // Normalize Windows paths for comparison (case-insensitive, forward slashes)
  const normRoot = path.normalize(statusResp.result.root).toLowerCase();
  const normExpected = path.normalize(TEST_DIR).toLowerCase();
  assert.equal(normRoot, normExpected, `Expected root ${TEST_DIR}, got ${statusResp.result.root}`);
  console.log('✓ workspace.status confirmed root matches C:\\Users\\miski\\Desktop\\test.');

  // 3. workspace.create_file
  console.log('Creating selam.md via workspace.create_file...');
  const selamContent = 'Merhaba! Bu, test dizini icerisinde olusturulmus otomatik bir selamlama dosyasidir. Sistem sorunsuz calismaktadir.';
  const createResp = await client.request('workspace.create_file', {
    path: 'selam.md',
    content: selamContent,
    overwrite: true,
  });
  console.log('create_file response:', JSON.stringify(createResp));
  assert.equal(createResp.ok, true, `create_file failed: ${JSON.stringify(createResp.error)}`);
  console.log('✓ workspace.create_file completed.');

  // 4. workspace.apply_patch for a new file
  console.log('Creating selam_patch.md via workspace.apply_patch...');
  const patchContent = 'Yama motoru ile olusturulan dosya.';
  const patchResp = await client.request('workspace.apply_patch', {
    path: 'selam_patch.md',
    expected_revision: 0,
    old_text: '',
    new_text: patchContent,
  });
  console.log('apply_patch response:', JSON.stringify(patchResp));
  assert.equal(patchResp.ok, true, `apply_patch failed: ${JSON.stringify(patchResp.error)}`);
  console.log('✓ workspace.apply_patch completed.');

  // 5. workspace.read_file
  console.log('Reading selam.md via workspace.read_file...');
  const readResp = await client.request('workspace.read_file', {
    path: 'selam.md',
  });
  console.log('read_file response:', JSON.stringify(readResp));
  assert.equal(readResp.ok, true, `read_file failed: ${JSON.stringify(readResp.error)}`);
  assert.ok(readResp.result.content.includes('Merhaba! Bu, test dizini icerisinde olusturulmus'));
  console.log('✓ workspace.read_file returned correct content.');

  // 6. workspace.search_code
  console.log('Searching code for "selamlama"...');
  const searchResp = await client.request('workspace.search_code', {
    query: 'selamlama',
  });
  console.log('search_code response:', JSON.stringify(searchResp));
  assert.equal(searchResp.ok, true, `search_code failed: ${JSON.stringify(searchResp.error)}`);
  assert.ok(searchResp.result.matches.length > 0, 'Expected at least 1 match');
  assert.ok(searchResp.result.matches.some(m => m.path.includes('selam.md')), 'Match should be in selam.md');
  console.log('✓ workspace.search_code found match in selam.md.');

  // 7. workspace.list_dir
  console.log('Listing directory via workspace.list_dir on "."...');
  const listResp = await client.request('workspace.list_dir', { path: '.' });
  console.log('list_dir response:', JSON.stringify(listResp));
  assert.equal(listResp.ok, true, `list_dir failed: ${JSON.stringify(listResp.error)}`);
  assert.ok(Array.isArray(listResp.result.entries), 'Expected entries array');
  const entryNames = listResp.result.entries.map(e => e.name);
  for (const expectedFile of ['selam.md', 'selam_patch.md', 'test.md', 'webbrain-workspace.exe']) {
    assert.ok(
      entryNames.includes(expectedFile),
      `Expected ${expectedFile} in list_dir entries, got: ${entryNames.join(', ')}`
    );
  }
  console.log('✓ workspace.list_dir returned selam.md, selam_patch.md, test.md, webbrain-workspace.exe.');

  // 8. workspace.glob
  console.log('Finding files via workspace.glob with pattern "*.md"...');
  const globResp = await client.request('workspace.glob', { pattern: '*.md' });
  console.log('glob response:', JSON.stringify(globResp));
  assert.equal(globResp.ok, true, `glob failed: ${JSON.stringify(globResp.error)}`);
  assert.ok(Array.isArray(globResp.result.matches), 'Expected matches array');
  for (const expectedMd of ['selam.md', 'selam_patch.md', 'test.md']) {
    assert.ok(
      globResp.result.matches.some(m => m.endsWith(expectedMd) || m === expectedMd),
      `Expected ${expectedMd} in glob matches, got: ${globResp.result.matches.join(', ')}`
    );
  }
  console.log('✓ workspace.glob returned matching .md files.');

  // 9. workspace.run_command with "dir"
  console.log('Executing command "dir" via workspace.run_command...');
  const cmdDirResp = await client.request('workspace.run_command', { command: 'dir' });
  console.log('run_command "dir" response:', JSON.stringify(cmdDirResp));
  assert.equal(cmdDirResp.ok, true, `run_command "dir" failed: ${JSON.stringify(cmdDirResp.error)}`);
  assert.equal(cmdDirResp.result.exitCode, 0, `Expected exitCode 0, got ${cmdDirResp.result.exitCode}`);
  assert.ok(cmdDirResp.result.stdout && cmdDirResp.result.stdout.length > 0, 'Expected non-empty stdout from "dir"');
  console.log('✓ workspace.run_command "dir" exited with 0 and non-empty stdout.');

  // 10. workspace.run_command with "cmd /c dir"
  console.log('Executing command "cmd /c dir" via workspace.run_command...');
  const cmdCmdDirResp = await client.request('workspace.run_command', { command: 'cmd /c dir' });
  console.log('run_command "cmd /c dir" response:', JSON.stringify(cmdCmdDirResp));
  assert.equal(cmdCmdDirResp.ok, true, `run_command "cmd /c dir" failed: ${JSON.stringify(cmdCmdDirResp.error)}`);
  assert.equal(cmdCmdDirResp.result.exitCode, 0, `Expected exitCode 0, got ${cmdCmdDirResp.result.exitCode}`);
  assert.ok(cmdCmdDirResp.result.stdout && cmdCmdDirResp.result.stdout.length > 0, 'Expected non-empty stdout from "cmd /c dir"');
  console.log('✓ workspace.run_command "cmd /c dir" exited with 0 and non-empty stdout.');

  // 7. Verify files physically on disk
  console.log('Verifying physical files on disk in C:\\Users\\miski\\Desktop\\test...');

  assert.ok(fs.existsSync(selamDiskPath), `File missing on disk: ${selamDiskPath}`);
  assert.ok(fs.existsSync(patchDiskPath), `File missing on disk: ${patchDiskPath}`);

  const diskSelamContent = fs.readFileSync(selamDiskPath, 'utf8');
  const diskPatchContent = fs.readFileSync(patchDiskPath, 'utf8');

  // Normalize CRLF for string check
  assert.ok(
    diskSelamContent.replace(/\r\n/g, '\n').includes(selamContent.replace(/\r\n/g, '\n')),
    `Disk content mismatch for selam.md: got "${diskSelamContent}"`
  );
  assert.ok(
    diskPatchContent.replace(/\r\n/g, '\n').includes(patchContent.replace(/\r\n/g, '\n')),
    `Disk content mismatch for selam_patch.md: got "${diskPatchContent}"`
  );

  console.log('✓ Both files are physically verified on disk!');
  console.log('==============================================');
  console.log('  ALL LIVE DESKTOP TESTS PASSED SUCCESSFULLY! ');
  console.log('==============================================');

  client.close();
}

run().catch((err) => {
  console.error('LIVE TEST FAILED:', err);
  process.exit(1);
});
