import api, { AudioPlayer as UniAudioPlayer, defaultHostConfig, FfiEqFilterKind, FfiError, initializeHost } from "./generated/kithara_ffi";

async function main() {
  const memory = new WebAssembly.Memory({ initial: 128, maximum: 1024, shared: true });
  const { default: init, AudioPlayer } = await import(new URL("./generated/wasm-bindgen/index.js", import.meta.url).href);
  await init({ module_or_path: "/generated/wasm-bindgen/index_bg.wasm", memory });
  api.initialize();

  let premature = false;
  try {
    UniAudioPlayer.newWeb();
  } catch (error) {
    premature = FfiError.NotInitialized.instanceOf(error);
  }
  if (!premature) throw new Error("generated player constructor accepted an uninitialized host");

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
  const generatedPlayer = UniAudioPlayer.newWeb();
  generatedPlayer.setEqLayout([{ kind: FfiEqFilterKind.Peaking, gainDb: 3, frequency: 1000, qFactor: 0.7 }]);
  if (generatedPlayer.eqBandCount() !== 1 || generatedPlayer.eqGain(0) !== 3) {
    throw new Error("generated player EQ layout did not reach owner readback");
  }
  if (!(generatedPlayer instanceof UniAudioPlayer)) throw new Error("generated player has no owned handle");
  generatedPlayer.uniffiDestroy();
  const player = new AudioPlayer();
  let rejectedBand = false;
  try {
    player.setEqGain(player.eqBandCount(), 3);
  } catch (_) {
    rejectedBand = true;
  }
  if (!rejectedBand || player.eqGain(0) !== 0) throw new Error("invalid EQ band changed readback");
  player.setEqGain(0, 9);
  if (player.eqGain(0) !== 6) throw new Error("EQ readback did not match clamped owner value");
  player.resetEq();
  if (player.eqGain(0) !== 0) throw new Error("EQ reset did not update readback");
  player.free();
  if (memory.buffer.byteLength > 64 * 1024 * 1024) throw new Error("Wasm memory bound exceeded");
  document.body.textContent = `PASS product-host defaults validation lifecycle EQ mutation; memory=${memory.buffer.byteLength}`;
}

main().catch(error => { document.body.textContent = `FAIL ${error.stack ?? error}`; });
