import { defineConfig } from "astro/config";
import { deptheosIntegration } from "./src/integrations/deptheos.ts";

export default defineConfig({
  site: "https://deptheos.com",
  trailingSlash: "always",
  // Keep false so Astro does not strip whitespace between adjacent inline elements (checked by scripts/smoke.ts).
  compressHTML: false,
  devToolbar: { enabled: false },
  server: { port: 5173 },
  build: {
    format: "directory",
    assets: "assets",
  },
  integrations: [deptheosIntegration()],
  vite: {
    build: {
      target: "es2022",
    },
  },
});
