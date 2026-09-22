import api, { defaultHostConfig, FfiError, initializeHost } from "./generated/kithara_ffi";

async function main() {
  const memory = new WebAssembly.Memory({ initial: 128, maximum: 1024, shared: true });
  const { default: init } = await import(new URL("./generated/wasm-bindgen/index.js", import.meta.url).href);
  await init({ module_or_path: "/generated/wasm-bindgen/index_bg.wasm", memory });
  api.initialize();

  const defaults = defaultHostConfig();
  if (defaults.sampleRateHint !== 44_100 || Math.abs(defaults.limiter.ceiling - 0.98) > 1e-6 || defaults.limiter.releaseMs !== 50) {
    throw new Error(`wrong Rust host defaults: ${JSON.stringify(defaults)}`);
  }
  let invalid = false;
  try {
    initializeHost({ ...defaults, limiter: { ...defaults.limiter, ceiling: 1.5 } });
  } catch (error) {
    invalid = FfiError.InvalidArgument.instanceOf(error);
  }
  if (!invalid) throw new Error("invalid limiter was accepted");
  invalid = false;
  try {
    initializeHost({ ...defaults, limiter: { ...defaults.limiter, releaseMs: -1 } });
  } catch (error) {
    invalid = FfiError.InvalidArgument.instanceOf(error);
  }
  if (!invalid) throw new Error("invalid release was accepted");

  initializeHost(defaults);
  let repeated = false;
  try {
    initializeHost(defaults);
  } catch (error) {
    repeated = FfiError.AlreadyInitialized.instanceOf(error);
  }
  if (!repeated) throw new Error("repeated host initialization was accepted");
  if (memory.buffer.byteLength > 64 * 1024 * 1024) throw new Error("Wasm memory bound exceeded");
  document.body.textContent = `PASS product-host defaults validation lifecycle; memory=${memory.buffer.byteLength}`;
}

main().catch(error => { document.body.textContent = `FAIL ${error.stack ?? error}`; });
