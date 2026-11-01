import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react()],

  build: {
    // 桌面端 dist 走本地 file:// 加载并打进 exe，没有网络往返，拆 chunk 换不来实际收益。
    // 单包 ~1.1MB 里约 1MB 是 antd 组件与样式；实测拆成 antd vendor 后业务包降到 113kB，
    // 但 antd 那个包仍是 1MB、告警照旧，反而多一层 complexity，故只把阈值调到实际水位。
    chunkSizeWarningLimit: 1200,
  },

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
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
