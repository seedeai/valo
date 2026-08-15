import { defineConfig } from "vitest/config";

/** Node-only unit tests for the parts of the shim that need no GPU. Pixel
 *  parity against a real browser lives in @valo/conformance. */
export default defineConfig({
  test: {
    environment: "node",
    include: ["tests/**/*.test.ts"],
  },
});
