const WebSocket = require('ws');
const ws = new WebSocket('ws://127.0.0.1:19830');
let chunks = [];
let done = false;

ws.on('open', () => {
  console.log('[CONNECTED]');
  ws.send(JSON.stringify({type: "session:create", sessionId: "test-claude-002", agentType: "claude"}));
});

ws.on('message', (data) => {
  const msg = JSON.parse(data.toString());
  const t = msg.type;

  if (t === 'status') {
    const s = msg.status || '';
    if (s.startsWith('session_created')) {
      console.log('[SESSION CREATED] claude');
      console.log('[CHAT] Say hello in one sentence');
      ws.send(JSON.stringify({
        type: "chat",
        sessionId: "test-claude-002",
        message: "Say hello in one sentence",
        agentType: "claude"
      }));
    }
  } else if (t === 'chunk') {
    const c = msg.content || '';
    chunks.push(c);
    process.stdout.write(c);
  } else if (t === 'done') {
    console.log(`\n[DONE] total chunks: ${chunks.length}`);
    done = true;
    ws.send(JSON.stringify({type: "session:close", sessionId: "test-claude-002", agentType: "claude"}));
    setTimeout(() => { ws.close(); process.exit(0); }, 1000);
  } else if (t === 'error') {
    console.log('[ERROR]', JSON.stringify(msg));
    done = true;
    setTimeout(() => { ws.close(); process.exit(1); }, 1000);
  }
});

ws.on('error', (err) => { console.error('[WS ERROR]', err.message); });

setTimeout(() => {
  if (!done) { console.log('\n[TIMEOUT 120s]'); process.exit(1); }
}, 120000);
