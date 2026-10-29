import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  // @lyflow/editor 的 peer 依赖与宿主共用同一份实例
  resolve: { dedupe: ["react", "react-dom", "@xyflow/react", "three", "zustand"] },
  optimizeDeps: { entries: ["index.html"] },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
});
