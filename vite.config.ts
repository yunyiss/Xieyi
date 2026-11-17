import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react(), tailwindcss()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. 忽略构建产物与离线依赖。
      //
      // `src-tauri/target` 现在有 20+ GB、`vendor/` 有 800 MB。它们既不参与
      // 前端构建，也不该进文件监听——Tailwind v4 的 dev 编译会因此多花
      // 几十秒，导致 WebView 迟迟等不到 CSS（表现为白屏）。
      ignored: ["**/src-tauri/**", "**/vendor/**", "**/docs/**"],
    },
  },
}));
