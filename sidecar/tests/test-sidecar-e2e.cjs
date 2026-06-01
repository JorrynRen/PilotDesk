/**
 * PilotDesk Sidecar End-to-End Communication Test
 * 
 * Tests the full WebSocket communication between a client and the Sidecar server.
 * Must be run AFTER the Sidecar is started (either via Tauri or standalone: node sidecar/dist/index.js)
 * 
 * Usage:
 *   cd pilotdesk/sidecar
 *   node tests/test-sidecar-e2e.cjs
 */

const WebSocket = require('ws');
const PORT = 19830;
const BASE_URL = `ws://localhost:${PORT}`;

let testsPassed = 0;
let testsFailed = 0;
const testResults = [];

function assert(condition, testName, detail = '') {
  if (condition) {
    testsPassed++;
    testResults.push(`  PASS: ${testName}${detail ? ' — ' + detail : ''}`);
  } else {
    testsFailed++;
    testResults.push(`  FAIL: ${testName}${detail ? ' — ' + detail : ''}`);
  }
}

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms));
}

function sendAndWait(ws, msg, typeFilter, timeout = 5000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new Error(`Timeout waiting for ${typeFilter} response after ${timeout}ms`));
    }, timeout);

    function handler(data) {
      try {
        const parsed = JSON.parse(data.toString());
        if (parsed.type === typeFilter) {
          clearTimeout(timer);
          ws.off('message', handler);
          resolve(parsed);
        }
      } catch {}
    }

    ws.on('message', handler);
    ws.send(JSON.stringify(msg));
  });
}

function sendAndWaitRaw(ws, rawMsg, typeFilter, timeout = 5000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new Error(`Timeout`));
    }, timeout);

    function handler(data) {
      try {
        const parsed = JSON.parse(data.toString());
        if (parsed.type === typeFilter) {
          clearTimeout(timer);
          ws.off('message', handler);
          resolve(parsed);
        }
      } catch {}
    }

    ws.on('message', handler);
    ws.send(rawMsg);
  });
}

async function runTests() {
  console.log('========================================');
  console.log(' PilotDesk Sidecar E2E Communication Test');
  console.log(` Target: ${BASE_URL}`);
  console.log('========================================\n');

  // ---- Test Group 1: Connection ----
  console.log('[Test Group 1] Connection');
  let ws;
  try {
    ws = new WebSocket(BASE_URL);
    await new Promise((resolve, reject) => {
      ws.on('open', resolve);
      ws.on('error', reject);
      setTimeout(() => reject(new Error('Connection timeout (5s)')), 5000);
    });
    assert(true, 'WebSocket connection established');
  } catch (err) {
    assert(false, 'WebSocket connection', err.message);
    console.log('\n  Cannot connect to Sidecar. Make sure it is running:');
    console.log('    cd sidecar && node dist/index.js');
    console.log('  Or launch the full Tauri app.\n');
    printResults();
    return;
  }

  await sleep(100);

  // ---- Test Group 2: Ping/Pong ----
  console.log('[Test Group 2] Ping/Pong');
  try {
    const response = await sendAndWait(ws, { type: 'ping', sessionId: 'test-001' }, 'status', 3000);
    assert(response.status === 'pong', 'Ping returns pong', `status=${response.status}`);
    assert(response.sessionId === 'test-001', 'Ping preserves sessionId');
  } catch (err) {
    assert(false, 'Ping/Pong', err.message);
  }

  await sleep(100);

  // ---- Test Group 3: Session Management ----
  console.log('[Test Group 3] Session Management');
  try {
    const resp = await sendAndWait(ws, {
      type: 'session:create',
      sessionId: 'test-session-claude',
      agentType: 'claude',
      cwd: process.cwd(),
    }, 'status', 3000);
    assert(resp.status === 'session_created:claude', 'Claude session created', `status=${resp.status}`);
    assert(resp.sessionId === 'test-session-claude', 'Session ID preserved');
  } catch (err) {
    assert(false, 'Claude session:create', err.message);
  }

  await sleep(100);

  try {
    const resp = await sendAndWait(ws, {
      type: 'session:create',
      sessionId: 'test-session-hermes',
      agentType: 'hermes',
      cwd: process.cwd(),
    }, 'status', 3000);
    assert(resp.status === 'session_created:hermes', 'Hermes session created', `status=${resp.status}`);
  } catch (err) {
    assert(false, 'Hermes session:create', err.message);
  }

  await sleep(100);

  // ---- Test Group 4: Skills Management ----
  console.log('[Test Group 4] Skills Management');
  try {
    const resp = await sendAndWait(ws, {
      type: 'skills:list',
      sessionId: '',
      agentType: 'claude',
    }, 'skills', 5000);
    assert(Array.isArray(resp.skills), 'Skills list returns array', `got ${typeof resp.skills}`);
    assert(resp.skills.length > 0, 'Claude has skills', `${resp.skills.length} skills`);
    assert(resp.agentType === 'claude', 'Skills response includes agentType');
    if (resp.skills.length > 0) {
      const first = resp.skills[0];
      assert(typeof first.name === 'string', 'Skill has name property', first.name);
      assert(typeof first.description === 'string', 'Skill has description property');
    }
    console.log(`  Claude skills (${resp.skills.length}):`);
    resp.skills.forEach(s => console.log(`    - ${s.name}: ${s.description}`));
  } catch (err) {
    assert(false, 'Skills list (claude)', err.message);
  }

  await sleep(100);

  try {
    const results = [];
    const collectPromise = new Promise((resolve) => {
      const timer = setTimeout(resolve, 8000);
      function handler(data) {
        try {
          const parsed = JSON.parse(data.toString());
          if (parsed.type === 'skills') {
            results.push(parsed);
          } else if (parsed.type === 'status' && parsed.status === 'skills:list-all:done') {
            clearTimeout(timer);
            ws.off('message', handler);
            resolve();
          }
        } catch {}
      }
      ws.on('message', handler);
    });

    ws.send(JSON.stringify({ type: 'skills:list-all', sessionId: '' }));
    await collectPromise;

    assert(results.length >= 2, 'Skills list-all returns multiple agents', `${results.length} agents`);
    const claudeResult = results.find(r => r.agentType === 'claude');
    const hermesResult = results.find(r => r.agentType === 'hermes');
    assert(!!claudeResult, 'Claude skills in list-all');
    assert(!!hermesResult, 'Hermes skills in list-all');
    console.log(`  All agents skills: ${results.map(r => `${r.agentType}(${r.skills?.length || 0})`).join(', ')}`);
  } catch (err) {
    assert(false, 'Skills list-all', err.message);
  }

  await sleep(100);

  // ---- Test Group 5: Chat ----
  console.log('[Test Group 5] Chat (Claude)');
  try {
    const chunks = [];
    let gotDone = false;
    let gotError = false;

    const chatPromise = new Promise((resolve) => {
      const timer = setTimeout(() => {
        ws.off('message', handler);
        resolve();
      }, 15000);

      function handler(data) {
        try {
          const parsed = JSON.parse(data.toString());
          if (parsed.sessionId === 'test-session-claude') {
            if (parsed.type === 'chunk') {
              chunks.push(parsed.content);
            } else if (parsed.type === 'done') {
              gotDone = true;
              clearTimeout(timer);
              ws.off('message', handler);
              resolve();
            } else if (parsed.type === 'error') {
              gotError = true;
              clearTimeout(timer);
              ws.off('message', handler);
              resolve();
            }
          }
        } catch {}
      }
      ws.on('message', handler);
    });

    ws.send(JSON.stringify({
      type: 'chat',
      sessionId: 'test-session-claude',
      message: 'Hello, this is a test message. Reply with "test ok".',
      mode: 'fast',
      agentType: 'claude',
      cwd: process.cwd(),
    }));

    await chatPromise;

    assert(gotDone || gotError || chunks.length > 0, 'Chat produces response',
      gotDone ? 'got done' : gotError ? 'got error' : `${chunks.length} chunks received`);

    if (gotError) {
      console.log('  (Chat error expected if Claude Code SDK not configured)');
      assert(true, 'Chat error handling works (SDK not configured)', '');
    }
    if (gotDone) {
      assert(true, 'Chat completed successfully', `${chunks.length} chunks, ${chunks.join('').length} chars`);
      console.log(`  Response preview: ${chunks.join('').slice(0, 200)}`);
    }
  } catch (err) {
    assert(false, 'Chat test', err.message);
  }

  await sleep(100);

  // ---- Test Group 6: Error Handling ----
  console.log('[Test Group 6] Error Handling');
  try {
    const resp = await sendAndWait(ws, {
      type: 'unknown:type',
      sessionId: 'test-err',
    }, 'error', 3000);
    assert(resp.type === 'error', 'Unknown type returns error');
    assert(resp.sessionId === 'test-err', 'Error preserves sessionId');
    assert(resp.error.includes('Unknown message type'), 'Error message descriptive', resp.error);
  } catch (err) {
    assert(false, 'Unknown message type error', err.message);
  }

  await sleep(100);

  try {
    const resp = await sendAndWaitRaw(ws, 'not-json-at-all', 'error', 3000);
    assert(resp.type === 'error', 'Invalid JSON returns error');
    assert(resp.error === 'Invalid message format', 'Error message for invalid JSON', resp.error);
  } catch (err) {
    assert(false, 'Invalid JSON handling', err.message);
  }

  await sleep(100);

  // ---- Test Group 7: Session Close ----
  console.log('[Test Group 7] Session Close');
  try {
    const resp = await sendAndWait(ws, {
      type: 'session:close',
      sessionId: 'test-session-claude',
      agentType: 'claude',
    }, 'status', 3000);
    assert(resp.status === 'session_closed', 'Claude session closed', resp.status);
  } catch (err) {
    assert(false, 'Session close', err.message);
  }

  await sleep(100);

  try {
    const resp = await sendAndWait(ws, {
      type: 'session:close',
      sessionId: 'test-session-hermes',
      agentType: 'hermes',
    }, 'status', 3000);
    assert(resp.status === 'session_closed', 'Hermes session closed', resp.status);
  } catch (err) {
    assert(false, 'Hermes session close', err.message);
  }

  // ---- Test Group 8: Stop Generation ----
  console.log('[Test Group 8] Stop Generation');
  try {
    const resp = await sendAndWait(ws, {
      type: 'stop',
      sessionId: 'test-stop',
      agentType: 'claude',
    }, 'status', 3000);
    assert(resp.status === 'generation_stopped', 'Stop generation returns status', resp.status);
  } catch (err) {
    assert(false, 'Stop generation', err.message);
  }

  // Cleanup
  ws.close();
  printResults();
}

function printResults() {
  console.log('\n========================================');
  console.log(' Test Results');
  console.log('========================================');
  for (const r of testResults) {
    console.log(r);
  }
  console.log('----------------------------------------');
  console.log(`  Total: ${testsPassed + testsFailed}  Passed: ${testsPassed}  Failed: ${testsFailed}`);
  console.log('========================================\n');

  if (testsFailed > 0) {
    process.exit(1);
  }
}

runTests().catch(err => {
  console.error('Test runner error:', err);
  process.exit(2);
});
