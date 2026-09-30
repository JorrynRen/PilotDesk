import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { readFileSync } from "node:fs";

// ──────────────────────────────────────────────
// 版本号单一来源
// 事实来源是根 package.json 的 version，注入为 import.meta.env.VITE_APP_VERSION，
// 前端各组件统一从这里读取，避免再出现散落的硬编码版本号。
// 仍需人工保持一致的三处：package.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml
// （发版清单见 README「发布流程」）。
// ──────────────────────────────────────────────
const pkg = JSON.parse(
  readFileSync(new URL("./package.json", import.meta.url), "utf-8"),
) as { version: string };

const host = process.env.TAURI_DEV_HOST;

export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  define: {
    // 文本替换，构建时静态内联；开发期同样可用
    "import.meta.env.VITE_APP_VERSION": JSON.stringify(pkg.version),
  },
  server: {
    port: 1420,
    strictPort: true,
    host: host || "127.0.0.1",
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
}));
