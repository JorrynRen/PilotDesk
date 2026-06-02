
async function test() {
  console.log("Loading SDK...");
  const dynamicImport = new Function('modulePath', 'return import(modulePath)');
  const sdk = await dynamicImport('@anthropic-ai/claude-code/sdk.mjs');
  console.log("SDK loaded, exports:", Object.keys(sdk));
  console.log("query type:", typeof sdk.query);

  try {
    console.log("Calling query...");
    const response = await sdk.query({
      prompt: "Say hello in one sentence",
      options: {
        cwd: process.cwd(),
      },
    });
    console.log("Response type:", typeof response);
    console.log("Response constructor:", response?.constructor?.name);
    console.log("Response keys:", response ? Object.keys(response).slice(0, 10) : "null");

    if (response && typeof response[Symbol.asyncIterator] === 'function') {
      console.log("Response is async iterable, iterating...");
      let count = 0;
      for await (const event of response) {
        count++;
        console.log(`Event ${count}: type=${event.type}, keys=${Object.keys(event).join(',')}`);
        if (event.type === 'assistant' && event.message) {
          const content = event.message.content;
          if (Array.isArray(content)) {
            for (const block of content) {
              if (block.type === 'text') {
                console.log("TEXT:", block.text.substring(0, 200));
              }
            }
          }
        }
        if (count >= 5) {
          console.log("(stopping after 5 events)");
          break;
        }
      }
      console.log("Total events:", count);
    } else if (response && typeof response.then === 'function') {
      console.log("Response is a promise, awaiting...");
      const result = await response;
      console.log("Result:", JSON.stringify(result).substring(0, 500));
    }
  } catch (err) {
    console.error("ERROR:", err.message);
    console.error("Stack:", err.stack?.substring(0, 300));
  }
}

test().catch(console.error);
