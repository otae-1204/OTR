import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5174,
    strictPort: true,
    // cargo 链接时 target 里的 exe 会被锁住;Vite 监视到它会在 Windows 上 EBUSY 退出
    watch: { ignored: ["**/src-tauri/target/**"] },
  },
  build: { target: "es2021", outDir: "dist" },
});
