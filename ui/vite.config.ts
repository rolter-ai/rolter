import react from "@vitejs/plugin-react";
import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  server: {
    port: Number(process.env.PORT) || 3000,
    proxy: {
      // proxy management api calls to rolter-control during dev
      "/api": "http://localhost:4001",
      // proxy playground /v1 calls to the rolter-gateway (data plane) during
      // dev so the browser stays same-origin (no CORS). in production the
      // control plane needs to reverse-proxy /gw/* to the gateway (follow-up).
      "/gw": {
        target: "http://localhost:4000",
        changeOrigin: true,
        rewrite: (p) => p.replace(/^\/gw/, ""),
        ws: true,
      },
    },
  },
  build: { outDir: "dist" },
});
