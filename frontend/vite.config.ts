import path from "node:path";
import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react-swc";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "");
  const apiTarget = env.VITE_API_TARGET || "http://127.0.0.1:8080";
  return {
    plugins: [react()],
    resolve: {
      alias: {
        "@": path.resolve(__dirname, "src"),
      },
    },
    build: {
      chunkSizeWarningLimit: 1000,
      rollupOptions: {
        output: {
          manualChunks(id) {
            // React and page-only CommonJS packages share this virtual helper.
            if (id === "\0commonjsHelpers.js") return "react-runtime";
            if (!id.includes("/node_modules/")) return;
            if (
              /\/node_modules\/(?:react|react-dom|react-router|react-router-dom|scheduler|use-sync-external-store|dequal)\//.test(
                id,
              )
            )
              return "react-runtime";
            if (id.includes("/prettier/")) return "transform-formatter";
            if (
              /\/(?:@codemirror|@lezer|@uiw\/react-codemirror|codemirror)\//.test(
                id,
              )
            )
              return "transform-editor";
            if (/\/(?:@ai-sdk|ai)\//.test(id)) return "playground-ai";
          },
        },
      },
    },
    server: {
      allowedHosts: ["localhost", "127.0.0.1", "::1"],
      proxy: {
        "/api": {
          target: apiTarget,
          changeOrigin: true,
        },
      },
    },
  };
});
