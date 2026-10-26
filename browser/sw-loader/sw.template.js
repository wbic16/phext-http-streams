// phext Service Worker loader (spec/bundle-v1.md). Generated into dist/phext-sw.js by build.mjs, which
// replaces __WASM_BASE64__ with the phext-stream-wasm module.
//
// Register at the site root:  navigator.serviceWorker.register('/phext-sw.js?bundle=/app.phext')
//
// On every navigation the worker fetches the bundle once and answers that page's requests from it:
// - inline resources stream into their Response as their bytes arrive (no waiting for the scroll to end);
// - external resources start fetching the moment the header scroll closes, before the HTML parser has
//   even seen them;
// - anything not in the bundle, or listed but never delivered, goes to the network as usual.

const WASM_BASE64 = '__WASM_BASE64__';
const BUNDLE_PATH = new URL(self.location.href).searchParams.get('bundle') || '/app.phext';
const SELF_PATH = new URL(self.location.href).pathname;

const wasmModule = new WebAssembly.Module(Uint8Array.from(atob(WASM_BASE64), (c) => c.charCodeAt(0)));

function deferred() {
  let resolve, reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

const isText = (t) => t.startsWith('text/') || t === 'application/json' || t === 'application/xml' || t === 'image/svg+xml';

class Resource {
  constructor(entry) {
    this.entry = entry;
    this.chunks = [];
    this.done = false;
    this.listeners = new Set();
    this.started = deferred(); // true once bytes start, false if they never will
  }
  push(bytes) {
    this.chunks.push(bytes);
    for (const c of this.listeners) c.enqueue(bytes);
  }
  close() {
    this.done = true;
    for (const c of this.listeners) c.close();
    this.listeners.clear();
  }
  // Each request gets its own stream: replay what has arrived, then follow live.
  stream() {
    return new ReadableStream({
      start: (c) => {
        for (const b of this.chunks) c.enqueue(b);
        if (this.done) c.close();
        else this.listeners.add(c);
      },
      cancel: () => {},
    });
  }
  response() {
    const type = this.entry.type + (isText(this.entry.type) ? '; charset=utf-8' : '');
    return new Response(this.stream(), { headers: { 'content-type': type, 'x-phext': this.entry.coord } });
  }
}

class BundleLoad {
  constructor(url) {
    this.headerReady = deferred();
    this.byPath = new Map();
    this.resources = [];
    this.prefetched = new Map();
    this.failed = false;
    const text = new TextDecoder();
    const view = (p, l) => new Uint8Array(this.memory.buffer, p, l);
    const imports = {
      phext: {
        header: (p, l) => this.onHeader(text.decode(view(p, l))),
        start: (i) => this.resources[i].started.resolve(true),
        data: (i, p, l) => this.resources[i].push(view(p, l).slice()),
        end: (i) => this.resources[i].close(),
        unlisted: () => {},
        missing: (i) => this.resources[i].started.resolve(false),
        error: (p, l) => this.fail(new Error(text.decode(view(p, l)))),
      },
    };
    this.wasm = new WebAssembly.Instance(wasmModule, imports).exports;
    this.memory = this.wasm.memory;
    this.reader = this.wasm.phext_reader_new();
    this.run(url).catch((e) => this.fail(e));
  }

  onHeader(t) {
    const lines = t.split('\n');
    lines.shift(); // magic, already checked by the reader
    for (const line of lines) {
      const f = line.split(/\s+/);
      if (f.length !== 3) continue;
      const [loc, media, path] = f;
      const base64 = media.endsWith(';base64');
      const entry = { coord: loc, type: base64 ? media.slice(0, -7) : media, path, inline: loc !== 'external' };
      const r = new Resource(entry);
      this.byPath.set(path, this.resources.length);
      this.resources.push(r);
      if (!entry.inline) {
        r.started.resolve(false);
        // start the fetch now; the page's request picks it up when the parser gets there
        this.prefetched.set(path, fetch(path, { credentials: 'same-origin' }).catch(() => null));
      }
    }
    this.headerReady.resolve(true);
  }

  fail(e) {
    if (this.failed) return;
    this.failed = true;
    console.warn('phext bundle failed; falling back to the network:', e);
    this.headerReady.resolve(false);
    for (const r of this.resources) {
      r.started.resolve(false);
      if (!r.done) for (const c of r.listeners) c.error(e);
    }
  }

  async run(url) {
    const res = await fetch(url, { credentials: 'same-origin' });
    if (!res.ok || !res.body) throw new Error(`bundle ${url}: HTTP ${res.status}`);
    const reader = res.body.getReader();
    let cap = 0;
    let ptr = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (value.length > cap) {
        if (cap) this.wasm.phext_free(ptr, cap);
        cap = Math.max(value.length, 64 * 1024);
        ptr = this.wasm.phext_alloc(cap);
      }
      new Uint8Array(this.memory.buffer, ptr, value.length).set(value);
      if (this.wasm.phext_reader_feed(this.reader, ptr, value.length) !== 0) return;
    }
    if (cap) this.wasm.phext_free(ptr, cap);
    // reports `missing` entries, or calls `error` (and so fail()) if the header never arrived
    this.wasm.phext_reader_finish(this.reader);
  }

  async respond(request, path) {
    if (!(await this.headerReady.promise)) return fetch(request);
    let i = this.byPath.get(path);
    if (i === undefined && path.endsWith('/')) i = this.byPath.get(path + 'index.html');
    if (i === undefined) return fetch(request);
    const r = this.resources[i];
    if (!r.entry.inline) {
      const p = this.prefetched.get(r.entry.path);
      this.prefetched.delete(r.entry.path);
      const hit = p && (await p);
      return hit && hit.ok ? hit : fetch(request);
    }
    return (await r.started.promise) ? r.response() : fetch(request);
  }
}

let current = null;

self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (e) => e.waitUntil(self.clients.claim()));

self.addEventListener('fetch', (e) => {
  const url = new URL(e.request.url);
  if (url.origin !== self.location.origin || e.request.method !== 'GET') return;
  if (url.pathname === BUNDLE_PATH || url.pathname === SELF_PATH || url.pathname.startsWith('/__')) return;
  if (e.request.mode === 'navigate') current = new BundleLoad(BUNDLE_PATH);
  if (!current) return;
  e.respondWith(current.respond(e.request, url.pathname));
});
