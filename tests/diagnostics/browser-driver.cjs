#!/usr/bin/env node
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const net = require('node:net');
const readline = require('node:readline');
const vm = require('node:vm');
const { spawn } = require('node:child_process');
const prelude = fs.readFileSync(`${__dirname}/browser-startup.js`, 'utf8');

function inject(html) {
  assert(/<head(?:\s[^>]*)?>/i.test(html), 'expected test HTML head');
  return html.replace(/<head(?:\s[^>]*)?>/i, head => head + `<script>${prelude}</script>`);
}

if (process.argv[2] === '--self-test') {
  new vm.Script(prelude);
  const html = '<!DOCTYPE html><html><head></head><body></body></html>';
  assert.equal(inject(html).replace(`<script>${prelude}</script>`, ''), html);
  assert(inject(html).startsWith('<!DOCTYPE html><html><head><script>'));
  assert.throws(() => inject('<html></html>'));
  const events = [];
  const context = {
    WebAssembly, BroadcastChannel: class { postMessage() {} }, console: { ...console },
    navigator: { sendBeacon: (_, body) => events.push(JSON.parse(body)), locks: { request() {} } },
    performance: { timeOrigin: 0, now: () => 0 },
    Blob: class { constructor(parts) { this.parts = parts; } },
    Worker: class { addEventListener() {} postMessage() {} }
  };
  vm.runInNewContext(prelude, context);
  const source = `self.onmessage = async () => { const shim = await import(url);
  shim.initSync({ module, memory, thread_stack_size: 1048576 });
  shim["wasm_safe_thread_entry_point"](work); }; // wasm_safe_thread:bootstrap`;
  const result = new context.Blob([source], { type: 'text/javascript' });
  assert.equal(result.parts.length, 2);
  new vm.Script(result.parts[0]);
  new vm.Script(result.parts.join(''));
  assert(result.parts[1].includes("__kithara_startup_trace('entry')"));
  assert.throws(() => new context.Blob(['wasm_safe_thread:bootstrap'], { type: 'text/javascript' }));
  assert(events.some(event => event.stage === 'probe-installed'));
  const previous = context.console.debug;
  context.console.debug = (...args) => previous(...args);
  context.console.debug('probe console replacement');
  const module = new context.WebAssembly.Module(new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]));
  assert(module instanceof context.WebAssembly.Module);
  assert(new context.WebAssembly.Instance(module) instanceof context.WebAssembly.Instance);
  const channel = new context.BroadcastChannel();
  channel.name = 'kithara-events';
  channel.postMessage({ scope: 'analysis' });
  channel.postMessage({ scope: 'analysis' });
  assert.equal(events.filter(event => event.stage === 'first-publication').length, 1);
  assert(events.some(event => event.stage === 'compile-sync-end'));
  assert(events.some(event => event.stage === 'instantiate-sync-end'));
  process.stdout.write('Browser startup probe self-test passed\n');
  process.exit(0);
}

async function main() {
  const output = process.env.KITHARA_BOOTSTRAP_TRACE;
  assert(output, 'KITHARA_BOOTSTRAP_TRACE is required');
  fs.mkdirSync(require('node:path').dirname(output), { recursive: true });
  const record = (stage, data = {}) => fs.appendFileSync(output, JSON.stringify({
    received: Date.now(), pid: process.pid, package: process.env.CARGO_PKG_NAME, stage, ...data
  }) + '\n');
  const args = process.argv.slice(2);
  const portIndex = args.findIndex(arg => arg.startsWith('--port='));
  assert(portIndex >= 0, 'expected chromedriver --port argument');
  const originalPort = Number(args[portIndex].split('=')[1]);
  assert(originalPort > 0);
  const reserve = net.createServer();
  await new Promise(resolve => reserve.listen(0, '127.0.0.1', resolve));
  const realPort = reserve.address().port;
  await new Promise(resolve => reserve.close(resolve));
  args[portIndex] = `--port=${realPort}`;
  const resources = [];

  async function resourceServer(original) {
    assert.equal(original.hostname, '127.0.0.1');
    const server = http.createServer((request, response) => {
      if (request.url === '/__kithara_startup.jsonl' && request.method === 'POST') {
        const chunks = [];
        request.on('data', chunk => chunks.push(chunk));
        request.on('end', () => {
          record('browser', { origin: original.href, ...JSON.parse(Buffer.concat(chunks)) });
          response.writeHead(204); response.end();
        });
        return;
      }
      const started = Date.now();
      const upstream = http.request({ hostname: original.hostname, port: original.port,
        path: request.url, method: request.method,
        headers: { ...request.headers, host: original.host, 'accept-encoding': 'identity' }
      }, result => {
        record('resource-start', { origin: original.href, url: request.url, status: result.statusCode });
        if ((result.headers['content-type'] || '').includes('text/html')) {
          assert(!result.headers['content-encoding']);
          const chunks = [];
          result.on('data', chunk => chunks.push(chunk));
          result.on('end', () => {
            const headers = { ...result.headers };
            delete headers['content-length'];
            response.writeHead(result.statusCode, headers);
            response.end(inject(Buffer.concat(chunks).toString()));
          });
        } else {
          response.writeHead(result.statusCode, result.headers);
          result.pipe(response);
        }
        result.on('end', () => record('resource-end', { origin: original.href, url: request.url, started }));
      });
      upstream.on('error', error => {
        record('resource-error', { error: error.message }); response.writeHead(502); response.end();
      });
      request.pipe(upstream);
    });
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    resources.push(server);
    return server;
  }

  const proxy = http.createServer((request, response) => {
    const chunks = [];
    request.on('data', chunk => chunks.push(chunk));
    request.on('end', async () => {
      let body = Buffer.concat(chunks);
      if (request.method === 'POST' && request.url.endsWith('/url')) {
        const payload = JSON.parse(body);
        const original = new URL(payload.url);
        const server = await resourceServer(original);
        const rewritten = new URL(original);
        rewritten.port = String(server.address().port);
        payload.url = rewritten.href;
        record('navigation', { original: original.href, rewritten: rewritten.href });
        body = Buffer.from(JSON.stringify(payload));
      }
      const upstream = http.request({ hostname: '127.0.0.1', port: realPort, path: request.url,
        method: request.method, headers: { ...request.headers, host: `127.0.0.1:${realPort}`, 'content-length': body.length }
      }, result => { response.writeHead(result.statusCode, result.headers); result.pipe(response); });
      upstream.on('error', error => {
        record('driver-error', { error: error.message }); response.writeHead(502); response.end();
      });
      upstream.end(body);
    });
  });
  const driver = spawn('chromedriver', args);
  driver.stdout.pipe(process.stdout);
  driver.stderr.pipe(process.stderr);
  let listening = false;
  readline.createInterface({ input: driver.stdout }).on('line', line => {
    if (!listening && line.includes('was started successfully')) {
      listening = true;
      proxy.listen(originalPort, '127.0.0.1');
      record('driver-ready', { originalPort, realPort });
    }
  });
  driver.on('error', error => { record('driver-spawn-error', { error: error.message }); process.exit(2); });
  driver.on('exit', (code, signal) => {
    record('driver-exit', { code, signal });
    proxy.closeAllConnections();
    for (const server of resources) server.closeAllConnections();
    process.exit(code || (signal ? 1 : 0));
  });
  for (const signal of ['SIGTERM', 'SIGINT']) process.on(signal, () => driver.kill(signal));
}
main().catch(error => { process.stderr.write(error.stack + '\n'); process.exit(2); });
