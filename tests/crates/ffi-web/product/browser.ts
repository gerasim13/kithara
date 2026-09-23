import api, { AudioPlayer as UniAudioPlayer, defaultHostConfig, FfiCrossfadeCurve, FfiEqFilterKind, FfiError, initializeHost, tickHost } from "./generated/kithara_ffi";

async function main() {
  const memory = new WebAssembly.Memory({ initial: 128, maximum: 1024, shared: true });
  const { default: init, AudioPlayer } = await import(new URL("./generated/wasm-bindgen/kithara-ffi.js", import.meta.url).href);
  await init({ module_or_path: "/generated/wasm-bindgen/kithara-ffi_bg.wasm", memory });
  api.initialize();

  let prematureTick = false;
  try {
    tickHost();
  } catch (error) {
    prematureTick = FfiError.NotInitialized.instanceOf(error);
  }
  if (!prematureTick) throw new Error("generated host tick accepted an uninitialized host");

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
  let oversizedLayout = false;
  try {
    generatedPlayer.setEqLayout(Array(65).fill({ kind: FfiEqFilterKind.Peaking, gainDb: 0, frequency: 1000, qFactor: 0.7 }));
  } catch (error) {
    oversizedLayout = FfiError.InvalidArgument.instanceOf(error);
  }
  if (!oversizedLayout || generatedPlayer.eqBandCount() !== 1) {
    throw new Error("oversized EQ layout changed the player owner");
  }
  const crossfade = { duration: 2.5, curve: FfiCrossfadeCurve.Linear, depth: 0.75, position: 0.4 };
  const pump = setInterval(() => tickHost(), 16);
  const events = new BroadcastChannel("kithara-events");
  const workerApplied = new Promise<void>((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("worker did not apply crossfade settings")), 10_000);
    events.onmessage = ({ data }) => {
      if (data.kind !== "CrossfadeSettingsChanged") return;
      clearTimeout(timeout);
      if (data.duration !== crossfade.duration || data.curve !== "Linear" || data.depth !== crossfade.depth || data.position !== Math.fround(crossfade.position)) {
        reject(new Error(`worker applied wrong crossfade settings: ${JSON.stringify(data)}`));
      } else {
        resolve();
      }
    };
  });
  generatedPlayer.setCrossfadeSettings(crossfade);
  const appliedCrossfade = generatedPlayer.crossfadeSettings();
  if (appliedCrossfade.duration !== crossfade.duration || appliedCrossfade.curve !== crossfade.curve || appliedCrossfade.depth !== crossfade.depth || appliedCrossfade.position !== Math.fround(crossfade.position)) {
    throw new Error(`generated crossfade settings did not reach owner readback: ${JSON.stringify(appliedCrossfade)}`);
  }
  let invalidCrossfade = false;
  try {
    generatedPlayer.setCrossfadeSettings({ ...crossfade, depth: 1.5 });
  } catch (error) {
    invalidCrossfade = FfiError.InvalidArgument.instanceOf(error);
  }
  if (!invalidCrossfade || JSON.stringify(generatedPlayer.crossfadeSettings()) !== JSON.stringify(appliedCrossfade)) {
    throw new Error("invalid crossfade changed the player owner");
  }
  await workerApplied;
  clearInterval(pump);
  events.close();
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
  document.body.textContent = `PASS product-host defaults validation lifecycle EQ and worker-applied crossfade mutation; memory=${memory.buffer.byteLength}`;
}

main().catch(error => { document.body.textContent = `FAIL ${error.stack ?? error}`; });
