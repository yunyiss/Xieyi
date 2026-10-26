import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

/**
 * 前端测试只跑纯逻辑与组件，不启动 Tauri：
 * `lib/ipc` 在测试里统一用 `vi.mock` 打桩，事件与命令都变成可控的假实现。
 */
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
