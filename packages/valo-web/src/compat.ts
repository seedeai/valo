import initializeWasm, * as bindings from "../wasm-compat/valo_web.js";

export * from "../wasm-compat/valo_web.js";
// The boundary takes integers for every mode; these are their names.
export * from "./enums.js";

/**
 * The compat build of the raw API: the same binary with wgpu's WebGL2 backend
 * compiled in. Where WebGPU exists it is used; where it does not, rendering
 * falls back to WebGL2 instead of failing.
 *
 * What the fallback costs, and why this is not the default entry:
 * - the GLSL transpiler roughly doubles the wasm download for everyone;
 * - WebGL is one context per canvas, so on the fallback path `Device.attach`
 *   accepts a single canvas — create one `Device` per canvas instead;
 * - only this raw API is served: the Canvas2D layer stays on `@valo/web`,
 *   which requires WebGPU.
 */

let initialization: Promise<typeof bindings> | undefined;

/** Load the compat WebAssembly module once and return its raw retained API. */
export function initializeValo(
  moduleOrPath?: Parameters<typeof initializeWasm>[0],
): Promise<typeof bindings> {
  initialization ??= initializeWasm(moduleOrPath).then(() => bindings);
  return initialization;
}
