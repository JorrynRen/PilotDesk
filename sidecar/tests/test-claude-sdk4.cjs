
async function test() {
  const dynamicImport = new Function('modulePath', 'return import(modulePath)');
  const sdk = await dynamicImport('@anthropic-ai/claude-code/sdk.mjs');

  const claudeExe = 'C:\\Users\\Administrator\\AppData\\Roaming\\npm\\claude.cmd';

  try {
    console.log("Calling sdk.query...");
    const response = await sdk.query({
      prompt: "Reply with just: Hello World",
      options: {
        cwd: "E:\\LingXi Workspace\\20260530-10-40-29-689\\pilotdesk",
        pathToClaudeCodeExecutable: claudeExe,
      },
    });

    console.log("Got response, iterating...");
    let count = 0;
    for await (const event of response) {
      count++;
      console.log(`[${count}] type=${event.type}`);
      if (event.type === 'assistant') {
        const content = event.message?.content || [];
        for (const block of content) {
          if (block.type === 'text') {
            console.log(`  TEXT: ${block.text.substring(0, 300)}`);
          }
        }
      }
    }
    console.log("FINISH. Total events:", count);
  } catch (err) {
    console.error("CAUGHT:", err.message);
  }
  process.exit(0);
}

test();
