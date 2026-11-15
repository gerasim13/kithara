const bootstrapProbe = (data) => console.info("KITHARA_BOOTSTRAP " + JSON.stringify(data));

if (typeof Worker === "function" && typeof Blob === "function") {
  const NativeBlob = Blob;
  const NativeWorker = Worker;
  globalThis.Blob = class extends NativeBlob {
    constructor(parts, options) {
      if (options?.type === "text/javascript") {
        parts = parts.map((part) => {
          if (typeof part !== "string" || !part.includes("let __wstWorkerId")) return part;
          const before = part;
          const replace = (from, to) => {
            if (part.split(from).length !== 2) throw new Error("bootstrap probe marker mismatch: " + from);
            part = part.replace(from, to);
          };
          replace("const shim = await navigator.locks.request(", "__kbStage('lock-wait');\n        const shim = await navigator.locks.request(");
          replace("const shim = await import(url);", "__kbStage('lock-enter');\n            const shim = await import(url);\n            __kbStage('import-ready');");
          replace("shim.initSync({ module, memory, thread_stack_size: 1048576 });", "__kbStage('wasm-init-begin');\n            shim.initSync({ module, memory, thread_stack_size: 1048576 });\n            __kbStage('wasm-init-ready');");
          replace("// Call the entry point", "__kbStage('entry-begin');\n        // Call the entry point");
          if (part === before) throw new Error("bootstrap probe added no stages");
          return "const __kbStage = phase => self.postMessage({ __kithara_bootstrap: { name: __wstWorkerName, id: __wstWorkerId, phase } });\n" + part;
        });
      }
      super(parts, options);
    }
  };
  globalThis.Worker = class extends NativeWorker {
    constructor(url, options) {
      super(url, options);
      this.addEventListener("message", ({ data }) => {
        if (data?.__kithara_bootstrap) bootstrapProbe(data.__kithara_bootstrap);
      });
    }
  };
}

export class HostDevice {
  constructor(rate, root, script, module, memory, channels) {
    this.context = new AudioContext({ sampleRate: rate });
    this.owner = null;
    this.node = null;
    this.sent = false;
    this.connected = false;
    this.closed = false;
    this.failure = undefined;
    this.listeners = [];
    this.initialize(root, script, module, memory, channels).catch((error) => { if (!this.closed) this.fail(error); });
  }

  get sampleRate() { return this.context.sampleRate; }
  get acquired() { return !!this.booted && !this.closed && !this.failure; }
  get currentTime() { return this.context.currentTime; }
  get ready() { return this.connected && !this.closed && !this.failure && this.context.state === "running"; }
  get error() { return this.failure; }

  async initialize(root, script, module, memory, channels) {
    bootstrapProbe({ owner: "audio", phase: "import-begin" });
    this.bindings = await import(root);
    bootstrapProbe({ owner: "audio", phase: "import-ready" });
    const url = URL.createObjectURL(new Blob([
      `import * as bindgen from ${JSON.stringify(root)};\n`, script,
    ], { type: "text/javascript" }));
    try {
      bootstrapProbe({ owner: "audio", phase: "lock-wait" });
      await navigator.locks.request("wasm_safe_thread:bootstrap", async () => {
        bootstrapProbe({ owner: "audio", phase: "lock-enter" });
        bootstrapProbe({ owner: "audio", phase: "module-begin" });
        await this.context.audioWorklet.addModule(url);
        bootstrapProbe({ owner: "audio", phase: "module-ready" });
        if (this.closed) return;
        await new Promise((resolve, reject) => {
          this.node = new AudioWorkletNode(this.context, "KitharaOutput", {
            numberOfInputs: 0, numberOfOutputs: 1,
            channelCount: channels, outputChannelCount: [channels],
            processorOptions: [module, memory],
          });
          this.node.onprocessorerror = (event) => { reject(event); this.fail(event.message); };
          this.node.port.onmessage = ({ data }) => {
            switch (data.kind) {
              case "ready":
                resolve();
                this.booted = true;
                if (this.closed) this.node.port.postMessage({ kind: "stop" });
                else this.transfer();
                break;
              case "installed":
                if (this.closed) { this.node.port.postMessage({ kind: "stop" }); break; }
                try {
                  this.node.connect(this.context.destination);
                  this.connected = true;
                  this.enableResume();
                } catch (error) { this.fail(error); }
                break;
              case "error": reject(new Error(data.message)); this.fail(data.message); break;
              case "stopped":
                reject(new Error("output stopped before bootstrap completed"));
                this.sent = false;
                this.connected = false;
                this.closed = true;
                this.removeListeners();
                this.node.disconnect();
                this.node.port.close();
                this.closeContext();
                break;
              default: reject(new Error("invalid output worklet message")); this.fail("invalid output worklet message");
            }
          };
        });
      });
    } finally {
      URL.revokeObjectURL(url);
    }
  }

  install(owner) {
    if (this.closed || this.failure || this.owner || this.sent) {
      owner.free();
      throw new Error(this.failure || "output installation is not vacant");
    }
    this.owner = owner;
    this.transfer();
  }

  transfer() {
    if (!this.booted || !this.owner || this.closed) return;
    const handle = this.owner.__destroy_into_raw();
    this.owner = null;
    try {
      this.node.port.postMessage({ kind: "install", handle });
      this.sent = true;
    } catch (error) {
      this.bindings.HostOutput.__wrap(handle).free();
      this.fail(error);
    }
  }

  enableResume() {
    if (this.context.state === "running") return;
    const resume = () => {
      this.context.resume().then(() => this.removeListeners()).catch((error) => console.error("Audio autoresume failed", error));
    };
    for (const name of ["click", "contextmenu", "auxclick", "dblclick", "mousedown", "mouseup", "pointerup", "touchend", "keydown", "keyup"]) {
      document.addEventListener(name, resume);
      this.listeners.push([name, resume]);
    }
  }

  removeListeners() {
    for (const [name, listener] of this.listeners) document.removeEventListener(name, listener);
    this.listeners.length = 0;
  }

  fail(error) {
    if (this.failure === undefined) this.failure = String(error);
    this.close();
  }

  closeContext() {
    if (this.context.state !== "closed") this.context.close().catch((error) => console.error("Audio context close failed", error));
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    this.removeListeners();
    if (this.owner) { this.owner.free(); this.owner = null; }
    if (this.node) this.node.port.postMessage({ kind: "stop" });
    else this.closeContext();
  }
}
