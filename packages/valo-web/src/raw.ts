import initializeWasm, * as bindings from "../wasm/valo_web.js";

export * from "../wasm/valo_web.js";

let initialization: Promise<typeof bindings> | undefined;

/** Load Valo's WebAssembly module once and return its raw retained API. */
export function initializeValo(
  moduleOrPath?: Parameters<typeof initializeWasm>[0],
): Promise<typeof bindings> {
  initialization ??= initializeWasm(moduleOrPath).then(() => bindings);
  return initialization;
}
