// Generates the benchmark sites and their four delivery variants under bench/out/<fixture>/<variant>:
//   files    plain files; the browser discovers and fetches each one (gzip'd text)
//   preload  same files, plus Link: preload for every subresource (and 103 Early Hints over HTTP/2)
//   inline   one HTML document with CSS, JS, images and data inlined (the strongest one-response
//            baseline), mirroring the default bundle: anything it leaves external stays separate here too
//   phext    same files plus app.phext(.gz) built by phext-bundle and the Service Worker loader
//            (plus the phext-* bundler variants below)
// Every page sets window.__done when its scripts have run, images have loaded, and data has arrived.
// Usage: node bench/fixtures.mjs
import { execFileSync } from 'node:child_process';
import { mkdirSync, rmSync, writeFileSync, readFileSync, copyFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gzipSync } from 'node:zlib';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..');
const OUT = join(here, 'out');

function rng(seed) {
  let s = seed >>> 0 || 1;
  return () => {
    s ^= s << 13;
    s ^= s >>> 17;
    s ^= s << 5;
    return (s >>> 0) / 4294967296;
  };
}

// code-like filler that gzips roughly like real JS/CSS (~3-4x)
const WORDS = ('const let return function render state props value index length update element node ' +
  'children style class component effect callback promise resolve then await async import export ' +
  'default map filter reduce push slice object array string number true false null undefined this ' +
  'window document query selector event listener handler option config theme layout grid flex').split(' ');
function filler(r, bytes) {
  let s = '';
  while (s.length < bytes) {
    const w = WORDS[Math.floor(r() * WORDS.length)];
    s += r() < 0.15 ? `${w}${Math.floor(r() * 1000)} ` : `${w} `;
  }
  return s.slice(0, bytes);
}

// valid 24-bit BMP with noise pixels (incompressible, like real photo bytes)
function bmp(r, w, h) {
  const row = Math.ceil((w * 3) / 4) * 4;
  const size = 54 + row * h;
  const b = Buffer.alloc(size);
  b.write('BM', 0);
  b.writeUInt32LE(size, 2);
  b.writeUInt32LE(54, 10);
  b.writeUInt32LE(40, 14);
  b.writeInt32LE(w, 18);
  b.writeInt32LE(h, 22);
  b.writeUInt16LE(1, 26);
  b.writeUInt16LE(24, 28);
  b.writeUInt32LE(row * h, 34);
  for (let i = 54; i < size; i++) b[i] = Math.floor(r() * 256);
  return b;
}

const FIXTURES = {
  // a small landing page: a few modules two levels deep, a few icons, one hero image
  small: { css: [3000, 2500], modules: { depth: 2, fanout: 2, bytes: 3000 }, icons: 4, heroes: 1, channels: 0 },
  // a typical SPA shell: ~25 modules three levels deep, ~40 requests total
  spa: { css: [6000, 6000, 4000, 4000], modules: { depth: 3, fanout: 3, bytes: 5000 }, icons: 10, heroes: 2, channels: 0 },
  // the phext-native case: 50 small data channels fetched in parallel once the app starts
  channels: { css: [2000], modules: { depth: 1, fanout: 1, bytes: 2000 }, icons: 0, heroes: 0, channels: 50 },
  // image-heavy: little text, 24 small images, one large one; this is where the flight budget decides
  icons: { css: [2000], modules: { depth: 1, fanout: 2, bytes: 2000 }, icons: 24, heroes: 1, channels: 0 },
};

// phext variants: same site, different bundler settings. `phext` is the shipped default: everything
// inline (binaries as base64), one-shot gzip by libdeflate -12. The budgeted variants keep the 8 KB
// per-binary cap the earlier defaults used, so they reproduce those bundles exactly.
const BUDGETED = (rounds) => ['--inline-max', '8192', '--budget-rounds', String(rounds)];
const PHEXT_VARIANTS = {
  phext: [],
  'phext-1rt': BUDGETED(1), // single first flight (14.6 KB)
  'phext-2rt': BUDGETED(2), // HTTP/2's request -> response -> request window (43.8 KB); the previous default
  'phext-3rt': BUDGETED(3), // three rounds (102 KB)
  'phext-text': ['--inline-max', '0'], // text only: every binary left to HTTP/2 multiplexing
  'phext-flush': ['--flush', 'all'], // everything inline, flate2 with a sync-flush after every scroll
  'phext-flate2': ['--gzip', 'flate2:9'], // everything inline, one-shot flate2 -9 instead of libdeflate -12
  'phext-br': ['--also', 'br:11'], // everything inline, served as Content-Encoding: br
  'phext-zstd': ['--also', 'zstd:19'], // everything inline, served as Content-Encoding: zstd
};

function build(name, spec) {
  const r = rng(name.length * 7919 + spec.icons * 31 + 1);
  const files = new Map();
  const put = (p, data) => files.set(p, Buffer.isBuffer(data) ? data : Buffer.from(data));

  // module graph: main -> level 1 -> level 2 ...; each module imports its children (discovery waterfall)
  const mods = []; // { name, children, body }
  const makeMod = (level, idx) => {
    const name = `m${level}_${idx}`;
    const children = [];
    if (level < spec.modules.depth) {
      for (let k = 0; k < spec.modules.fanout; k++) children.push(makeMod(level + 1, idx * spec.modules.fanout + k));
    }
    mods.push({ name, children, body: filler(r, spec.modules.bytes) });
    return name;
  };
  const top = [];
  for (let k = 0; k < spec.modules.fanout; k++) top.push(makeMod(1, k));
  // mods is in post-order: children before parents, which is also a valid inline evaluation order
  const value = (m) => `const ${m.name} = ((s) => s.length + ${m.children.map((c) => c).join(' + ') || 0})(${JSON.stringify(m.body)});\n`;
  for (const m of mods) {
    const imports = m.children.map((c) => `import { ${c} } from '/js/${c}.js';\n`).join('');
    put(`/js/${m.name}.js`, `${imports}export ${value(m)}`);
  }

  const css = spec.css.map((bytes, i) => {
    const sel = filler(r, bytes).split(' ').filter(Boolean);
    let s = '';
    for (let k = 0; s.length < bytes; k++) s += `.${sel[k % sel.length]}${k}{margin:${k % 7}px;color:#${(k * 2654435761 % 0xffffff).toString(16).padStart(6, '0')}}\n`;
    put(`/css/s${i}.css`, s);
    return `/css/s${i}.css`;
  });
  const icons = [];
  for (let i = 0; i < spec.icons; i++) {
    const sz = i % 2 ? 24 : 16;
    put(`/img/icon${i}.bmp`, bmp(r, sz, sz));
    icons.push(`/img/icon${i}.bmp`);
  }
  const heroes = [];
  for (let i = 0; i < spec.heroes; i++) {
    put(`/img/hero${i}.bmp`, bmp(r, 160, 120));
    heroes.push(`/img/hero${i}.bmp`);
  }
  const channels = [];
  for (let i = 0; i < spec.channels; i++) {
    const items = [];
    for (let k = 0; k < 20; k++) items.push({ id: k, t: filler(r, 50) });
    put(`/data/ch${i}.json`, JSON.stringify({ channel: i, items }));
    channels.push(`/data/ch${i}.json`);
  }

  const done = `
const imgs = [...document.images];
await Promise.all(imgs.map((i) => (i.complete ? 0 : new Promise((r) => { i.onload = i.onerror = r; }))));
window.__ok = ${top.join(' + ')} > 0 && imgs.every((i) => i.naturalWidth > 0);
window.__done = performance.now();
`;
  const channelLoad = (inline) =>
    channels.length
      ? inline
        ? `const ch = [...document.querySelectorAll('script[type="application/json"]')].map((s) => JSON.parse(s.textContent));\nif (ch.length !== ${channels.length}) throw new Error('channels');\n`
        : `const ch = await Promise.all(${JSON.stringify(channels)}.map((p) => fetch(p).then((r) => r.json())));\nif (ch.length !== ${channels.length}) throw new Error('channels');\n`
      : '';
  put('/js/main.js', `${top.map((t) => `import { ${t} } from '/js/${t}.js';\n`).join('')}${channelLoad(false)}${done}`);

  const body = (imgSrc) =>
    `<h1>${name}</h1>\n<p>${filler(r, 400)}</p>\n` +
    icons.map((p) => `<img src="${imgSrc(p)}" width=16 height=16 alt="">`).join('\n') +
    '\n' +
    heroes.map((p) => `<img src="${imgSrc(p)}" width=160 height=120 alt="">`).join('\n');
  put(
    '/index.html',
    `<!doctype html>\n<html><head><meta charset="utf-8"><title>${name}</title>\n` +
      css.map((p) => `<link rel="stylesheet" href="${p}">`).join('\n') +
      `\n<script type="module" src="/js/main.js"></script>\n</head><body>\n${body((p) => p)}\n</body></html>\n`,
  );

  return { files, css, mods, top, icons, heroes, channels, body, channelLoad, done, value };
}

function writeDir(dir, files, { gzip = true } = {}) {
  for (const [p, data] of files) {
    const f = join(dir, p);
    mkdirSync(dirname(f), { recursive: true });
    writeFileSync(f, data);
    if (gzip && /\.(html|css|js|json)$/.test(p)) writeFileSync(f + '.gz', gzipSync(data, { level: 9 }));
  }
}

const bundler = join(root, 'target/release', process.platform === 'win32' ? 'phext-bundle.exe' : 'phext-bundle');
execFileSync('cargo', ['build', '--quiet', '--release', '-p', 'phext-bundle'], { cwd: root, stdio: 'inherit' });
execFileSync('node', [join(root, 'browser/sw-loader/build.mjs')], { stdio: 'inherit' });

rmSync(OUT, { recursive: true, force: true });
const summary = {};
for (const [name, spec] of Object.entries(FIXTURES)) {
  const fx = build(name, spec);
  const base = join(OUT, name);

  writeDir(join(base, 'files'), fx.files);
  writeDir(join(base, 'preload'), fx.files);

  // phext: bundle the site in priority order (document, CSS, entry JS, modules breadth-first, data, icons)
  const bfs = [];
  const byName = new Map(fx.mods.map((m) => [m.name, m]));
  const queue = [...fx.top];
  while (queue.length) {
    const n = queue.shift();
    bfs.push(`/js/${n}.js`);
    queue.push(...byName.get(n).children);
  }
  const order = ['/index.html', ...fx.css, '/js/main.js', ...bfs, ...fx.channels, ...fx.icons, ...fx.heroes];
  const orderFile = join(base, 'order.txt');
  writeFileSync(orderFile, order.join('\n') + '\n');
  const srcDir = join(base, 'src');
  writeDir(srcDir, fx.files, { gzip: false });
  const reports = {};
  for (const [variant, flags] of Object.entries(PHEXT_VARIANTS)) {
    const dir = join(base, variant);
    writeDir(dir, fx.files);
    execFileSync(bundler, [srcDir, '-o', join(dir, 'app'), '--order', orderFile, '--quiet', ...flags]);
    copyFileSync(join(root, 'browser/sw-loader/dist/phext-sw.js'), join(dir, 'phext-sw.js'));
    copyFileSync(join(root, 'browser/sw-loader/dist/__phext-warm.html'), join(dir, '__phext-warm.html'));
    reports[variant] = JSON.parse(readFileSync(join(dir, 'app.report.json'), 'utf8'));
  }
  rmSync(srcDir, { recursive: true });
  const report = reports.phext;
  const phextDir = join(base, 'phext');

  // inline: the same inline/external split the bundler chose
  const b64 = new Set(report.assets.filter((a) => a.decision === 'base64').map((a) => a.path));
  const dataUri = (p) => (b64.has(p) ? `data:image/bmp;base64,${fx.files.get(p).toString('base64')}` : p);
  const script =
    fx.mods.map((m) => fx.value(m)).join('') + fx.channelLoad(true) + fx.done;
  const inlineHtml =
    `<!doctype html>\n<html><head><meta charset="utf-8"><title>${name}</title>\n` +
    fx.css.map((p) => `<style>${fx.files.get(p)}</style>`).join('\n') +
    `\n<script type="module">\n${script}</script>\n` +
    fx.channels.map((p) => `<script type="application/json">${fx.files.get(p)}</script>`).join('\n') +
    `</head><body>\n${fx.body(dataUri)}\n</body></html>\n`;
  const inlineFiles = new Map([['/index.html', Buffer.from(inlineHtml)]]);
  for (const h of fx.heroes) if (!b64.has(h)) inlineFiles.set(h, fx.files.get(h));
  for (const i of fx.icons) if (!b64.has(i)) inlineFiles.set(i, fx.files.get(i));
  writeDir(join(base, 'inline'), inlineFiles);

  // preload: every subresource, in the same priority order
  const as = (p) => (p.endsWith('.css') ? 'style' : p.endsWith('.js') ? 'modulepreload' : p.endsWith('.json') ? 'fetch' : 'image');
  const links = order.slice(1).map((p) =>
    as(p) === 'modulepreload' ? `<${p}>; rel=modulepreload` : `<${p}>; rel=preload; as=${as(p)}${as(p) === 'fetch' ? '; crossorigin' : ''}`,
  );
  writeFileSync(join(base, 'preload.links'), links.join('\n') + '\n');
  writeFileSync(join(base, 'phext.links'), readFileSync(join(phextDir, 'app.links')));

  const totalGz = [...fx.files].reduce((n, [p, d]) => n + (/\.(html|css|js|json)$/.test(p) ? gzipSync(d, { level: 9 }).length : d.length), 0);
  summary[name] = {
    requests: fx.files.size,
    filesGz: totalGz,
    inlineHtmlGz: gzipSync(inlineHtml, { level: 9 }).length,
  };
  for (const [variant, rep] of Object.entries(reports)) {
    summary[name][`${variant} gz/ext`] = `${rep.gzip_bytes}/${rep.external.length}`;
  }
}
writeFileSync(join(OUT, 'summary.json'), JSON.stringify(summary, null, 2));
console.table(summary);
