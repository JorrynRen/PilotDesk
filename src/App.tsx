import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

function App() {
  const [greetMsg, setGreetMsg] = useState("");
  const [theme, setTheme] = useState<"light" | "dark">("dark");

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
  }, [theme]);

  useEffect(() => {
    invoke<string>("greet", { name: "PilotDesk" })
      .then(setGreetMsg)
      .catch(console.error);
  }, []);

  return (
    <div className="min-h-screen flex flex-col" style={{ backgroundColor: "var(--bg-primary)", color: "var(--text-primary)" }}>
      {/* Title Bar Area */}
      <header className="flex items-center justify-between px-4 py-2" style={{ borderBottom: "1px solid var(--border)" }}>
        <div className="flex items-center gap-2">
          <div className="w-6 h-6 rounded" style={{ background: "linear-gradient(135deg, #5B7FFF, #8B5CF6)" }} />
          <h1 className="text-sm font-semibold">PilotDesk</h1>
        </div>
        <button
          onClick={() => setTheme(t => t === "dark" ? "light" : "dark")}
          className="text-xs px-2 py-1 rounded"
          style={{ border: "1px solid var(--border)", color: "var(--text-secondary)" }}
        >
          {theme === "dark" ? "☀️ Light" : "🌙 Dark"}
        </button>
      </header>

      {/* Main Content */}
      <main className="flex-1 flex items-center justify-center">
        <div className="text-center">
          <div className="w-16 h-16 mx-auto mb-4 rounded-2xl" style={{ background: "linear-gradient(135deg, #5B7FFF, #8B5CF6)" }} />
          <h2 className="text-xl font-bold mb-2">PilotDesk</h2>
          <p className="text-sm" style={{ color: "var(--text-secondary)" }}>
            {greetMsg || "Loading..."}
          </p>
          <p className="text-xs mt-1" style={{ color: "var(--text-secondary)" }}>
            Claude Code & Hermes Agent Unified Client
          </p>
        </div>
      </main>
    </div>
  );
}

export default App;
