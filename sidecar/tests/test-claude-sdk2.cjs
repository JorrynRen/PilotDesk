
async function test() {
  const dynamicImport = new Function('modulePath', 'return import(modulePath)');
  const sdk = await dynamicImport('@anthropic-ai/claude-code/sdk.mjs');

  try {
    const response = await sdk.query({
      prompt: "Say hello",
      options: {
        cwd: process.cwd(),
      },
    });

    let count = 0;
    for await (const event of response) {
      count++;
      console.log(`Event ${count}: type=${event.type}`);
      if (event.type === 'init') {
        console.log("  init:", JSON.stringify(event).substring(0, 300));
      } else if (event.type === 'assistant') {
        console.log("  assistant message:", JSON.stringify(event.message?.content || []).substring(0, 500));
        break; // Got the response
      } else if (event.type === 'error') {
        console.log("  ERROR:", JSON.stringify(event).substring(0, 500));
        break;
      }
    }
    console.log("Done. Events:", count);
  } catch (err) {
    console.error("MAIN ERROR:", err.message);
    // Check if there's a cause
    if (err.cause) console.error("CAUSE:", JSON.stringify(err.cause).substring(0, 500));
    if (err.stderr) console.error("STDERR:", err.stderr.substring(0, 500));
  }
}

test();
