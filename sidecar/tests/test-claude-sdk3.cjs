
async function test() {
  const dynamicImport = new Function('modulePath', 'return import(modulePath)');
  const sdk = await dynamicImport('@anthropic-ai/claude-code/sdk.mjs');

  // Try with explicit Claude Code executable path
  const claudeExe = 'C:\\Users\\Administrator\\AppData\\Roaming\\npm\\claude.cmd';
  console.log("Claude exe:", claudeExe);
  console.log("Exists:", require('fs').existsSync(claudeExe));

  try {
    const response = await sdk.query({
      prompt: "Say hello in one sentence",
      options: {
        cwd: "E:\\LingXi Workspace\\20260530-10-40-29-689\\pilotdesk",
        pathToClaudeCodeExecutable: claudeExe,
      },
    });

    let count = 0;
    for await (const event of response) {
      count++;
      const evtStr = JSON.stringify(event, (key, val) => {
        if (typeof val === 'string' && val.length > 100) return val.substring(0, 100) + '...';
        return val;
      });
      console.log(`Event ${count}: ${evtStr.substring(0, 400)}`);
      if (count > 20) break;
    }
    console.log("Done. Events:", count);
  } catch (err) {
    console.error("ERROR:", err.message);
    console.error("Full:", JSON.stringify({
      message: err.message,
      name: err.name,
      code: err.code,
      cause: err.cause,
    }).substring(0, 1000));
  }
}

test();
