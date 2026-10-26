// Static origin for the benchmark: HTTP/1.1 or HTTP/2 over TLS, precompressed gzip, optional
// Link: preload + 103 Early Hints on the document, and phext bundles served as application/vnd.phext.
import { readFileSync, statSync, existsSync } from 'node:fs';
import { join, normalize } from 'node:path';
import https from 'node:https';
import http2 from 'node:http2';

const TYPES = {
  html: 'text/html; charset=utf-8',
  css: 'text/css; charset=utf-8',
  js: 'text/javascript; charset=utf-8',
  mjs: 'text/javascript; charset=utf-8',
  json: 'application/json',
  svg: 'image/svg+xml',
  txt: 'text/plain; charset=utf-8',
  bmp: 'image/bmp',
  png: 'image/png',
  gif: 'image/gif',
  jpg: 'image/jpeg',
  woff2: 'font/woff2',
  wasm: 'application/wasm',
  phext: 'application/vnd.phext',
};

/**
 * @param {object} o
 * @param {string} o.root      directory to serve
 * @param {'h1'|'h2'} o.proto
 * @param {string[]} [o.links] Link header values sent on the document (and as 103 Early Hints)
 * @param {{key: Buffer, cert: Buffer}} o.tls
 */
export function startServer({ root, proto, links = [], tls, port = 0 }) {
  const cache = new Map();
  const load = (p) => {
    if (!cache.has(p)) cache.set(p, existsSync(p) && statSync(p).isFile() ? readFileSync(p) : null);
    return cache.get(p);
  };

  const stats = { requests: 0, paths: [], bundleEncoding: null };
  const handler = (req, res) => {
    stats.requests++;
    stats.paths.push(req.url);
    let path = decodeURIComponent(new URL(req.url, 'https://x').pathname);
    if (path.endsWith('/')) path += 'index.html';
    const file = normalize(join(root, path));
    if (!file.startsWith(normalize(root))) {
      res.writeHead(403).end();
      return;
    }
    const ext = path.split('.').pop();
    // precompressed siblings, preferring the smaller encodings when the client accepts them and the
    // variant ships them (only phext-br / phext-zstd do)
    const accept = req.headers['accept-encoding'] || '';
    let encoding = null;
    let encoded = null;
    for (const [token, suffix] of [['br', '.br'], ['zstd', '.zst'], ['gzip', '.gz']]) {
      if (new RegExp(`\\b${token}\\b`).test(accept) && (encoded = load(file + suffix))) {
        encoding = token;
        break;
      }
    }
    const body = encoded || load(file);
    if (path.endsWith('.phext')) stats.bundleEncoding = encoding;
    if (!body) {
      res.writeHead(404, { 'content-type': 'text/plain' }).end('not found');
      return;
    }
    const headers = {
      'content-type': TYPES[ext] || 'application/octet-stream',
      // cacheable, so Early Hints preloads can be reused; the runner clears the cache before every run
      'cache-control': 'max-age=300',
      'content-length': body.length,
    };
    if (encoding) headers['content-encoding'] = encoding;
    if (ext === 'html' && links.length && req.headers['sec-fetch-dest'] === 'document') {
      // Chrome acts on 103 over HTTP/2 and HTTP/3 only. Sending the same Links in both the 103 and the
      // final response makes Chrome fetch every resource twice, so each protocol gets one of them.
      if (proto === 'h2') res.writeEarlyHints({ link: links });
      else headers.link = links.join(', ');
    }
    res.writeHead(200, headers);
    res.end(body);
  };

  const server =
    proto === 'h2'
      ? http2.createSecureServer({ ...tls, allowHTTP1: false }, handler)
      : https.createServer({ ...tls, ALPNProtocols: ['http/1.1'] }, handler);
  if (proto === 'h1') server.keepAliveTimeout = 60_000;
  return new Promise((resolve) => {
    server.listen(port, '127.0.0.1', () =>
      resolve({
        port: server.address().port,
        stats,
        reset() {
          stats.requests = 0;
          stats.paths = [];
        },
        close: () =>
          new Promise((r) => {
            server.close(() => r());
            server.closeAllConnections?.();
            // http2 sessions don't have closeAllConnections; destroy lingering sockets
            for (const s of server._phextSockets || []) s.destroy();
          }),
      }),
    );
    server._phextSockets = new Set();
    server.on('connection', (s) => {
      server._phextSockets.add(s);
      s.on('close', () => server._phextSockets.delete(s));
    });
  });
}
