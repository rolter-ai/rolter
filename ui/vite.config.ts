import react from "@vitejs/plugin-react";
import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vite";

import { readAppVersion } from "./scripts/app-version";

// the Cargo workspace version is the one release-plz maintains; package.json
// carried an independent version that nothing bumped (#953)
const appVersion = readAppVersion(fileURLToPath(new URL("../Cargo.toml", import.meta.url)));

export default defineConfig({
  plugins: [react()],
  define: {
    __APP_VERSION__: JSON.stringify(appVersion),
  },
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  server: {
    port: Number(process.env.PORT) || 3000,
    proxy: {
      // proxy management api calls to rolter-control during dev. anchored on
      // the path segment: a bare "/api" prefix also swallowed the dashboard's
      // own /api-keys screen, which then rendered blank in dev
      "^/api/": "http://localhost:4001",
      // proxy playground /v1 calls to the rolter-gateway (data plane) during
      // dev so the browser stays same-origin (no CORS). in production the
      // control plane needs to reverse-proxy /gw/* to the gateway (follow-up).
      "^/gw/": {
        target: "http://localhost:4000",
        changeOrigin: true,
        rewrite: (p) => p.replace(/^\/gw/, ""),
        ws: true,
      },
    },
  },
  build: { outDir: "dist" },
});
