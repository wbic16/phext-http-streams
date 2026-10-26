// Drives the phext-stream-wasm exports from Node exactly the way phext-sw.js does, against the shared
// bundle vector and a gzip'd bundle split into tiny chunks. Usage: node browser/sw-loader/test-wasm.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gunzipSync, gzipSync } from 'node:zlib';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '../..');
const module = new WebAssembly.Module(readFileSync(join(root, 'target/wasm32-unknown-unknown/wasm/phext_stream_wasm.wasm')));

function decode(bytes, chunkSize) {
  const out = { header: null, bodies: new Map(), order: [], missing: [], errors: [] };
  let memory;
  const text = new TextDecoder();
  const view = (p, l) => new Uint8Array(memory.buffer, p, l);
  const imports = {
    phext: {
      header: (p, l) => { out.header = text.decode(view(p, l)); out.order.push('header'); },
      start: (i) => { out.bodies.set(i, []); out.order.push(`start ${i}`); },
      data: (i, p, l) => out.bodies.get(i).push(...view(p, l)),
      end: (i) => out.order.push(`end ${i}`),
      unlisted: () => {},
      missing: (i) => out.missing.push(i),
      error: (p, l) => out.errors.push(text.decode(view(p, l))),
    },
  };
  const w = new WebAssembly.Instance(module, imports).exports;
  memory = w.memory;
  const r = w.phext_reader_new();
  const cap = 64;
  const ptr = w.phext_alloc(cap);
  for (let i = 0; i < bytes.length; i += chunkSize) {
    const piece = bytes.subarray(i, Math.min(i + chunkSize, bytes.length));
    new Uint8Array(memory.buffer, ptr, piece.length).set(piece);
    if (w.phext_reader_feed(r, ptr, piece.length) !== 0) break;
  }
  w.phext_reader_finish(r);
  w.phext_free(ptr, cap);
  w.phext_reader_free(r);
  return out;
}

// 1. the shared vector, every chunk size up to the scratch buffer
const vector = readFileSync(join(root, 'core/test-vectors/bundle-basic.phext'));
for (const size of [1, 2, 3, 7, 64]) {
  const got = decode(vector, size);
  assert.deepEqual(got.errors, []);
  assert.match(got.header, /^phext-bundle 1\n/);
  assert.deepEqual(got.order, ['header', 'start 0', 'end 0', 'start 1', 'end 1', 'start 2', 'end 2']);
  assert.equal(Buffer.from(got.bodies.get(1)).toString(), 'img{display:block}\n');
  // the base64 GIF scroll comes out decoded
  assert.deepEqual(Buffer.from(got.bodies.get(2)), Buffer.from('R0lGODlhAQABAAAAACw=', 'base64'));
}

// 2. through gzip, as the browser delivers it
const gunzipped = gunzipSync(gzipSync(vector));
assert.deepEqual(decode(gunzipped, 5).order.length, 7);

// 3. errors surface through the import, not as traps
const bad = decode(Buffer.from('\x17phext-bundle 1\n'), 4);
assert.equal(bad.errors.length, 1);
assert.match(bad.errors[0], /expected the header/);
assert.match(decode(Buffer.alloc(0), 1).errors[0], /expected the header/);

console.log('wasm reader: all checks passed');
