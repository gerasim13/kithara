(() => {
  function observe(report) {
    for (const method of ['compile', 'compileStreaming', 'instantiate', 'instantiateStreaming']) {
      const original = WebAssembly[method];
      WebAssembly[method] = function (...args) {
        report(method + '-start', { compiledModule: args[0] instanceof WebAssembly.Module });
        const result = Reflect.apply(original, this, args);
        result.then(() => report(method + '-end'), error => report(method + '-error', { error: String(error) }));
        return result;
      };
    }
    for (const [name, stage] of [['Module', 'compile-sync'], ['Instance', 'instantiate-sync']]) {
      WebAssembly[name] = new Proxy(WebAssembly[name], {
        construct(target, args, newTarget) {
          report(stage + '-start');
          try {
            const result = Reflect.construct(target, args, newTarget);
            report(stage + '-end');
            return result;
          } catch (error) {
            report(stage + '-error', { error: String(error) });
            throw error;
          }
        }
      });
    }
    const request = navigator.locks.request;
    navigator.locks.request = function (...args) {
      if (args[0] !== 'wasm_safe_thread:bootstrap') return Reflect.apply(request, this, args);
      report('lock-requested');
      const callback = args[args.length - 1];
      args[args.length - 1] = function (...values) {
        report('lock-granted');
        return Reflect.apply(callback, this, values);
      };
      const result = Reflect.apply(request, this, args);
      result.then(() => report('lock-released'), error => report('lock-error', { error: String(error) }));
      return result;
    };
    for (const method of ['log', 'debug', 'info', 'warn', 'error']) {
      const wrap = original => function (...args) {
        for (const [needle, stage] of [
          ['web queue worker entered', 'queue-entry'],
          ['web queue owner task started', 'queue-task'],
          ['web queue registration returned', 'registration-returned'],
          ['Invoking test:', 'test-invoked']
        ]) {
          const message = args.find(value => typeof value === 'string' && value.includes(needle));
          if (message) report(stage, stage === 'test-invoked' ? { test: message } : {});
        }
        return Reflect.apply(original, this, args);
      };
      let wrapped = wrap(console[method]);
      Object.defineProperty(console, method, {
        configurable: true, enumerable: true,
        get: () => wrapped,
        set: value => { wrapped = wrap(value); }
      });
    }
    const seen = new Set();
    const post = BroadcastChannel.prototype.postMessage;
    BroadcastChannel.prototype.postMessage = function (...args) {
      const result = Reflect.apply(post, this, args);
      if (['kithara-reply', 'kithara-events'].includes(this.name) && !seen.has(this.name)) {
        seen.add(this.name);
        report('first-publication', { channel: this.name, scope: args[0]?.scope });
      }
      return result;
    };
  }

  const report = (stage, extra = {}) => {
    const sent = navigator.sendBeacon('/__kithara_startup.jsonl', JSON.stringify({
      at: performance.timeOrigin + performance.now(), realm: 'window', stage, ...extra
    }));
    if (!sent) console.error('STARTUP_TRACE_DROPPED', stage);
  };
  observe(report);
  if (typeof window !== 'undefined') {
    const prototype = BaseAudioContext.prototype;
    const descriptor = Object.getOwnPropertyDescriptor(prototype, 'audioWorklet');
    const worklets = new WeakSet();
    Object.defineProperty(prototype, 'audioWorklet', { ...descriptor, get() {
      const worklet = Reflect.apply(descriptor.get, this, []);
      if (!worklets.has(worklet)) {
        worklets.add(worklet);
        const add = worklet.addModule;
        worklet.addModule = function (...args) {
          report('worklet-import-start', { url: String(args[0]) });
          const result = Reflect.apply(add, this, args);
          result.then(() => report('worklet-import-end'), error => report('worklet-import-error', { error: String(error) }));
          return result;
        };
      }
      return worklet;
    } });
    globalThis.AudioWorkletNode = new Proxy(AudioWorkletNode, {
      construct(target, args, newTarget) {
        report('worklet-node-start');
        const node = Reflect.construct(target, args, newTarget);
        report('worklet-node-end');
        node.port.addEventListener('message', event => report('worklet-message', { kind: event.data?.kind }));
        return node;
      }
    });
    let previous = performance.now();
    setInterval(() => {
      const now = performance.now();
      if (now - previous > 1000) report('main-eventloop-gap', { elapsed: now - previous });
      previous = now;
    }, 250);
  }
  const workerPrelude = `const __kithara_startup_trace = (stage, extra = {}) => self.postMessage({
    __kitharaStartup: { at: performance.timeOrigin + performance.now(), stage, ...extra }
  });
  __kithara_startup_trace('script-received');
  (${observe.toString()})(__kithara_startup_trace);
  self.addEventListener('message', event => __kithara_startup_trace('module-received', {
    compiledModule: event.data[0] instanceof WebAssembly.Module
  }), { once: true });\n`;
  function replaceOnce(source, needle, replacement) {
    if (source.split(needle).length !== 2) throw new Error('startup probe anchor mismatch: ' + needle);
    return source.replace(needle, replacement);
  }
  const blob = globalThis.Blob;
  globalThis.Blob = new Proxy(blob, {
    construct(target, args, newTarget) {
      const [parts, options] = args;
      if (options?.type === 'text/javascript' && Array.isArray(parts)
          && parts.some(part => typeof part === 'string' && part.includes('wasm_safe_thread:bootstrap'))) {
        if (parts.length !== 1 || typeof parts[0] !== 'string') throw new Error('unexpected worker Blob');
        let source = parts[0];
        source = replaceOnce(source, 'const shim = await import(url);',
          "__kithara_startup_trace('import-start', {url}); const shim = await import(url); __kithara_startup_trace('import-end');");
        const init = 'shim.initSync({ module, memory, thread_stack_size: 1048576 });';
        source = replaceOnce(source, init,
          "__kithara_startup_trace('initSync-start'); " + init + " __kithara_startup_trace('initSync-end');");
        const entry = 'shim["wasm_safe_thread_entry_point"](work);';
        source = replaceOnce(source, entry,
          "__kithara_startup_trace('entry'); " + entry + " __kithara_startup_trace('entry-returned');");
        args = [[workerPrelude, source], options];
      }
      return Reflect.construct(target, args, newTarget);
    }
  });
  let next = 0;
  let nextModule = 0;
  const modules = new WeakMap();
  globalThis.Worker = new Proxy(globalThis.Worker, {
    construct(target, args, newTarget) {
      const worker = ++next;
      report('worker-create-start', { worker, url: String(args[0]) });
      const result = Reflect.construct(target, args, newTarget);
      report('worker-created', { worker });
      const post = result.postMessage;
      result.postMessage = function (...messages) {
        const message = messages[0];
        if (Array.isArray(message) && message[0] instanceof WebAssembly.Module) {
          if (!modules.has(message[0])) modules.set(message[0], ++nextModule);
          report('worker-module-sent', { worker, module: modules.get(message[0]), meta: message[3] });
        }
        return Reflect.apply(post, this, messages);
      };
      result.addEventListener('message', ({ data }) => {
        if (data?.__kitharaStartup) report('worker-stage', { worker, ...data.__kitharaStartup, realm: 'worker' });
        if (data?.__wasm_safe_thread_error) report('worker-error', { worker, error: data.__wasm_safe_thread_error });
        if (data?.__wasm_safe_thread_exit) report('worker-exit', { worker });
      });
      result.addEventListener('error', event => report('worker-error', { worker, error: event.message }));
      return result;
    }
  });
  report('probe-installed');
})();
