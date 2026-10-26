// TCP proxy that puts a modeled network path between Chrome and the origin:
// - fixed round-trip time, split evenly between directions;
// - a TCP handshake: client bytes leave only one RTT after the connection opens (SYN / SYN-ACK);
// - downstream slow start: at most `cwnd` bytes per round trip, starting at `initcwnd` and doubling after
//   every round that was window-limited (no loss, no slow-start-after-idle);
// - a downstream bottleneck of `mbps`.
// TLS passes through untouched, so its handshake pays these delays too. On Windows the default timer
// tick is ~15.6 ms, so delivery uses a spin-assisted scheduler for sub-millisecond precision.
import net from 'node:net';

const now = () => performance.now();

class Clock {
  constructor() {
    this.q = [];
    this.timer = null;
    this.spinning = false;
  }
  at(t, fn) {
    // keep the queue sorted by time; equal times keep insertion order
    let i = this.q.length;
    while (i > 0 && this.q[i - 1].t > t) i--;
    this.q.splice(i, 0, { t, fn });
    if (i === 0) this.wake();
  }
  wake() {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    if (!this.spinning) {
      this.spinning = true;
      setImmediate(() => this.loop());
    }
  }
  loop() {
    this.spinning = false;
    const t = now();
    while (this.q.length && this.q[0].t <= t) this.q.shift().fn();
    if (!this.q.length) return;
    const wait = this.q[0].t - now();
    if (wait > 20) {
      this.timer = setTimeout(() => {
        this.timer = null;
        this.wake();
      }, wait - 17);
    } else {
      this.spinning = true;
      setImmediate(() => this.loop());
    }
  }
}

export function startProxy({ listenPort = 0, targetPort, rttMs = 50, mbps = 50, initcwnd = 10 * 1460 }) {
  const clock = new Clock();
  const half = rttMs / 2;
  const msPerByte = 8 / (mbps * 1000);
  const stats = { connections: 0, bytesDown: 0, bytesUp: 0 };

  const server = net.createServer((client) => {
    stats.connections++;
    client.setNoDelay(true);
    const up = net.connect(targetPort, '127.0.0.1');
    up.setNoDelay(true);
    const opened = now();
    let lastUp = 0;
    let lastDown = 0;
    let closed = false;
    const kill = () => {
      if (closed) return;
      closed = true;
      client.destroy();
      up.destroy();
    };
    client.on('error', kill);
    up.on('error', kill);

    // client -> origin: wait out the handshake, then one-way delay
    client.on('data', (buf) => {
      stats.bytesUp += buf.length;
      const leave = Math.max(now(), opened + rttMs);
      lastUp = Math.max(leave + half, lastUp);
      clock.at(lastUp, () => !closed && up.write(buf));
    });
    client.on('end', () => clock.at(Math.max(lastUp, now()), () => !closed && up.end()));

    // origin -> client: slow start + bottleneck + one-way delay
    const queue = [];
    let cwnd = initcwnd;
    let roundStart = -Infinity;
    let roundBytes = 0;
    let lineFree = 0;
    let pumpPending = false;
    let upEnded = false;
    const pump = () => {
      pumpPending = false;
      while (queue.length) {
        const t = now();
        if (t >= roundStart + rttMs) {
          if (roundBytes >= cwnd) cwnd *= 2; // the last round used its whole window
          roundStart = t;
          roundBytes = 0;
        }
        const room = cwnd - roundBytes;
        if (room <= 0) {
          if (!pumpPending) {
            pumpPending = true;
            clock.at(roundStart + rttMs, pump);
          }
          return;
        }
        const head = queue[0];
        const piece = head.length <= room ? queue.shift() : head.subarray(0, room);
        if (piece !== head) queue[0] = head.subarray(room);
        roundBytes += piece.length;
        lineFree = Math.max(t, lineFree) + piece.length * msPerByte;
        lastDown = lineFree + half;
        stats.bytesDown += piece.length;
        clock.at(lastDown, () => !closed && client.write(piece));
      }
      if (upEnded) clock.at(Math.max(lastDown, now()), () => !closed && client.end());
    };
    up.on('data', (buf) => {
      queue.push(buf);
      if (!pumpPending) pump();
    });
    up.on('end', () => {
      upEnded = true;
      if (!pumpPending) pump();
    });
  });

  return new Promise((resolve) => {
    server.listen(listenPort, '127.0.0.1', () => {
      resolve({
        port: server.address().port,
        stats,
        reset() {
          stats.connections = 0;
          stats.bytesDown = 0;
          stats.bytesUp = 0;
        },
        close: () => new Promise((r) => server.close(() => r())),
      });
    });
  });
}
