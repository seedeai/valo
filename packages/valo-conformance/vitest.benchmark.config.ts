import { playwright } from "@vitest/browser-playwright";
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["tests/benchmark.browser.test.ts"],
    testTimeout: 120_000,
    hookTimeout: 120_000,
    reporters: ["verbose"],
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
    },
  },
});
