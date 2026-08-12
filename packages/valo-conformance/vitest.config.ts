import { playwright } from "@vitest/browser-playwright";
import { defineConfig } from "vitest/config";
import { compareCanvases } from "./src/browser-command.js";

const fuzzTimeLimit = Number(process.env.VALO_FUZZ_TIME_LIMIT ?? 20_000);

export default defineConfig({
  define: {
    __VALO_FUZZ_RUNS__: JSON.stringify(process.env.VALO_FUZZ_RUNS ?? ""),
    __VALO_FUZZ_SEED__: JSON.stringify(process.env.VALO_FUZZ_SEED ?? ""),
    __VALO_FUZZ_TIME_LIMIT__: JSON.stringify(process.env.VALO_FUZZ_TIME_LIMIT ?? ""),
  },
  test: {
    include: ["tests/**/*.browser.test.ts"],
    exclude: ["tests/benchmark.browser.test.ts"],
    testTimeout: Math.max(120_000, fuzzTimeLimit + 30_000),
    hookTimeout: 120_000,
    browser: {
      enabled: true,
      headless: true,
      provider: playwright({
        launchOptions: {
          channel: "chromium",
          args: ["--enable-unsafe-webgpu"],
        },
      }),
      instances: [{ browser: "chromium" }],
      viewport: { width: 400, height: 220 },
      screenshotFailures: false,
      commands: { compareCanvases },
    },
  },
});
