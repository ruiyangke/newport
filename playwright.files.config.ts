import { defineConfig } from "@playwright/test";
import base from "./playwright.config";
export default defineConfig({
  ...base,
  testMatch: "files.spec.ts",
  use: { ...base.use, baseURL: "http://127.0.0.1:1431" },
  webServer: {
    command: "npm run preview -- --port 1431 --strictPort",
    url: "http://127.0.0.1:1431",
    reuseExistingServer: false,
  },
});
