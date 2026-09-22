import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, readFile, realpath, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { extname, join, sep } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

// The xtask parent owns the process group, wall deadline, and sampled RSS limit.
const [directory, executable] = process.argv.slice(2);
const root = await realpath(directory);
const profile = await mkdtemp(join(tmpdir(), "kithara-sdk-"));
const types = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm" };
const server = createServer(async (request, response) => {
  try {
    const pathname = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
    const path = await realpath(join(root, pathname === "/" ? "index.html" : pathname));
    assert(path.startsWith(root + sep), "path outside SDK fixture");
    const content = await readFile(path);
    response.writeHead(200, {
      "Content-Type": types[extname(path)] ?? "application/octet-stream",
      "Cross-Origin-Opener-Policy": "same-origin",
      "Cross-Origin-Embedder-Policy": "require-corp",
      "Cache-Control": "no-store",
    });
    response.end(content);
  } catch {
    response.writeHead(404);
    response.end();
  }
});
let browser;
let socket;
try {
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  browser = spawn(executable, [
    "--remote-debugging-port=0", `--user-data-dir=${profile}`,
    "--disable-gpu", "--no-first-run", "--js-flags=--max-old-space-size=128", "about:blank",
  ], { stdio: ["ignore", "ignore", "inherit"] });
  let browserError;
  browser.on("error", error => { browserError = error; });
  let port;
  const started = performance.now();
  // Poll the browser's startup artifact; an exited browser cannot become ready.
  while (!port) {
    if (browserError) throw browserError;
    assert(browser.exitCode === null && browser.signalCode === null, "browser exited before startup");
    assert(performance.now() - started < 5000, "browser startup deadline");
    try {
      port = (await readFile(join(profile, "DevToolsActivePort"), "utf8")).split("\n")[0];
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
      await delay(25);
    }
  }
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const target = targets.find(target => target.type === "page");
  assert(target, "missing browser page");
  socket = new WebSocket(target.webSocketDebuggerUrl);
  await once(socket, "open");
  let sequence = 0;
  const pending = new Map();
  socket.addEventListener("message", event => {
    const message = JSON.parse(event.data);
    const waiter = pending.get(message.id);
    if (!waiter) return;
    pending.delete(message.id);
    if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
    else waiter.resolve(message.result);
  });
  socket.addEventListener("close", () => {
    for (const waiter of pending.values()) waiter.reject(new Error("browser connection closed"));
    pending.clear();
  });
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++sequence;
    pending.set(id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params }));
  });
  await call("Network.enable");
  await call("Network.setCacheDisabled", { cacheDisabled: true });
  await call("Page.navigate", { url: `http://127.0.0.1:${server.address().port}/` });
  let passed = false;
  while (performance.now() - started < 20000) {
    const result = await call("Runtime.evaluate", {
      expression: "document.body?.innerText", returnByValue: true,
    });
    const body = result.result.value ?? "";
    assert(!body.startsWith("FAIL"), body);
    if (body.startsWith("PASS main+worker") && body.endsWith("cross-thread-wake PASS")) {
      console.log(body);
      passed = true;
      break;
    }
    await delay(100);
  }
  assert(passed, "UniFFI browser contract deadline");
} finally {
  socket?.close();
  if (browser?.pid && browser.exitCode === null && browser.signalCode === null) {
    const exited = once(browser, "exit");
    browser.kill("SIGKILL");
    await exited;
  }
  server.closeAllConnections();
  server.close();
  await rm(profile, { recursive: true, force: true });
}
