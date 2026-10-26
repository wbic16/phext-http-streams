// Checks the network model against hand-computed timings. Usage: node bench/test-proxy.mjs
import assert from 'node:assert/strict';
import net from 'node:net';
import { startProxy } from './proxy.mjs';

// origin: answers any request byte with `size` bytes
function origin(size) {
  const s = net.createServer((c) => c.once('data', () => c.end(Buffer.alloc(size, 0x61))));
  return new Promise((r) => s.listen(0, '127.0.0.1', () => r(s)));
}

async function timeTransfer({ rttMs, mbps, size }) {
  const o = await origin(size);
  const p = await startProxy({ targetPort: o.address().port, rttMs, mbps });
  const t0 = performance.now();
  const result = await new Promise((resolve, reject) => {
    const c = net.connect(p.port, '127.0.0.1', () => c.write('x'));
    let first = null;
    let got = 0;
    c.on('data', (d) => {
      first ??= performance.now() - t0;
      got += d.length;
    });
    c.on('end', () => resolve({ first, last: performance.now() - t0, got }));
    c.on('error', reject);
  });
  await p.close();
  o.close();
  return result;
}

const tol = 4; // ms

// 1 byte: handshake (1 RTT) + request (1/2) + response (1/2) = 2 RTT
{
  const r = await timeTransfer({ rttMs: 50, mbps: 1000, size: 1 });
  assert.ok(Math.abs(r.first - 100) < tol, `first byte at ${r.first.toFixed(1)} ms, want ~100`);
}

// 100 KB at 1 Gbit/s: windows 14600, 29200, 58400 -> fits in 3 rounds: last byte ~ 100 + 2*50 = 200 ms
{
  const r = await timeTransfer({ rttMs: 50, mbps: 1000, size: 100_000 });
  assert.equal(r.got, 100_000);
  assert.ok(Math.abs(r.last - 200) < tol + 2, `last byte at ${r.last.toFixed(1)} ms, want ~200`);
}

// 14600 bytes (exactly initcwnd) arrive in the first flight: ~100 ms + serialization
{
  const r = await timeTransfer({ rttMs: 50, mbps: 1000, size: 14_600 });
  assert.ok(Math.abs(r.last - 100) < tol, `initcwnd flight done at ${r.last.toFixed(1)} ms, want ~100`);
}

// bandwidth-bound: 1 MB at 10 Mbit/s takes >= 800 ms of serialization
{
  const r = await timeTransfer({ rttMs: 20, mbps: 10, size: 1_000_000 });
  assert.ok(r.last > 840 && r.last < 1000, `1 MB at 10 Mbit/s took ${r.last.toFixed(1)} ms`);
}

console.log('proxy model: all checks passed');
