// Runs every fixture x protocol x variant x RTT through the network-model proxy in a fresh headless
// Chrome, and writes bench/results/<stamp>.json plus a Markdown summary.
//
// Usage: node bench/run.mjs [--fixtures small,spa,channels,icons] [--protos h1,h2] [--runs 6] [--mbps 50]
//          [--variants files,preload,inline,phext,phext-1rt,phext-2rt,phext-3rt,phext-text,phext-flush,phext-flate2,phext-br,phext-zstd]
//          [--rtt 20,50,100,200]
// Run `node bench/fixtures.mjs` first. Set CHROME to override the browser path.
import { execFileSync } from 'node:child_process';
import { createHash, X509Certificate } from 'node:crypto';
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import puppeteer from 'puppeteer-core';
import { startProxy } from './proxy.mjs';
import { startServer } from './server.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const arg = (name, def) => {
  const i = process.argv.indexOf(`--${name}`);
  return i > 0 ? process.argv[i + 1] : def;
};
const list = (name, def) => arg(name, def).split(',').filter(Boolean);
const FIXTURES = list('fixtures', 'small,spa,channels,icons');
const PROTOS = list('protos', 'h1,h2');
const VARIANTS = list('variants', 'files,preload,inline,phext,phext-1rt,phext-2rt,phext-3rt,phext-text,phext-flush,phext-flate2,phext-br,phext-zstd');
const RTTS = list('rtt', '20,50,100,200').map(Number);
const RUNS = Number(arg('runs', '6'));
const MBPS = Number(arg('mbps', '50'));
const CHROME = process.env.CHROME || 'C:/Program Files/Google/Chrome/Application/chrome.exe';

function ensureCerts() {
  const dir = join(here, 'certs');
  const key = join(dir, 'key.pem');
  const cert = join(dir, 'cert.pem');
  if (!existsSync(cert)) {
    mkdirSync(dir, { recursive: true });
    const openssl = process.env.OPENSSL || 'openssl';
    execFileSync(openssl, ['req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes', '-days', '365',
      '-subj', '/CN=localhost', '-addext', 'subjectAltName=DNS:localhost,IP:127.0.0.1', '-keyout', key, '-out', cert], { stdio: 'ignore' });
  }
  const pem = readFileSync(cert);
  const spki = createHash('sha256').update(new X509Certificate(pem).publicKey.export({ type: 'spki', format: 'der' })).digest('base64');
  return { tls: { key: readFileSync(key), cert: pem }, spki };
}

const { tls, spki } = ensureCerts();
const profiles = join(here, 'profiles');
rmSync(profiles, { recursive: true, force: true });

function launch(userDataDir) {
  return puppeteer.launch({
    executablePath: CHROME,
    headless: true,
    userDataDir,
    args: [
      `--ignore-certificate-errors-spki-list=${spki}`,
      '--no-first-run',
      '--no-default-browser-check',
      '--disable-background-networking',
      '--disable-component-update',
      '--disable-sync',
      '--disable-features=Translate,OptimizationHints,MediaRouter',
    ],
  });
}

async function warm(url, dir) {
  const browser = await launch(dir);
  const page = await browser.newPage();
  await page.goto(`${url}/__phext-warm.html`, { waitUntil: 'load' });
  await page.waitForFunction(() => document.title !== 'phext: installing', { timeout: 30_000 });
  const title = await page.title();
  await browser.close();
  if (title !== 'phext: ready') throw new Error(`Service Worker warm-up failed: ${title}`);
}

async function once(url, dir) {
  const browser = await launch(dir);
  try {
    const page = await browser.newPage();
    const cdp = await page.createCDPSession();
    await cdp.send('Network.clearBrowserCache');
    await page.goto(`${url}/`, { waitUntil: 'load', timeout: 60_000 });
    await page.waitForFunction(() => window.__done !== undefined, { timeout: 60_000 });
    // fast pages can finish before their first paint is recorded; FCP keeps its real timestamp
    await page.waitForFunction(() => performance.getEntriesByName('first-contentful-paint').length > 0, { timeout: 5_000 }).catch(() => {});
    return await page.evaluate(() => {
      const nav = performance.getEntriesByType('navigation')[0];
      const fcp = performance.getEntriesByName('first-contentful-paint')[0];
      return {
        done: window.__done,
        ok: window.__ok === true,
        fcp: fcp ? fcp.startTime : null,
        ttfb: nav.responseStart,
        sw: !!(navigator.serviceWorker && navigator.serviceWorker.controller),
      };
    });
  } finally {
    await browser.close();
  }
}

const median = (a) => {
  const s = [...a].sort((x, y) => x - y);
  return s.length % 2 ? s[(s.length - 1) / 2] : (s[s.length / 2 - 1] + s[s.length / 2]) / 2;
};
const pct = (a, p) => [...a].sort((x, y) => x - y)[Math.min(a.length - 1, Math.floor((a.length - 1) * p))];

const isPhext = (v) => v.startsWith('phext');
// `phext-nginx`: the bundle comes from real nginx + the phext module (servers/nginx-bench.sh start <fixture>),
// reached on PHEXT_NGINX_H2 / PHEXT_NGINX_H1 (default 18443 / 18444) instead of the Node origin.
const EXTERNAL = {
  'phext-nginx': { h2: Number(process.env.PHEXT_NGINX_H2 || 18443), h1: Number(process.env.PHEXT_NGINX_H1 || 18444) },
  // same nginx, pre-built bundle served statically (gzip_static): separates the module from the WSL/nginx path
  'phext-nginx-static': { h2: 18453, h1: 18454 },
};
const externalServer = (variant, proto) => ({
  port: EXTERNAL[variant][proto],
  stats: { requests: null, bundleEncoding: null },
  reset() {},
  close: async () => {},
});
// identity of what a phext variant actually serves: every encoded bundle it ships
const bundleHash = (fixture, v) => {
  if (!isPhext(v) || EXTERNAL[v]) return null;
  const h = createHash('sha256');
  for (const ext of ['gz', 'br', 'zst']) {
    const f = join(here, 'out', fixture, v, `app.phext.${ext}`);
    h.update(ext).update(existsSync(f) ? readFileSync(f) : Buffer.alloc(0));
  }
  return h.digest('hex');
};

const results = [];
for (const fixture of FIXTURES) {
  for (const proto of PROTOS) {
    for (const [vi, variant] of VARIANTS.entries()) {
      const root = join(here, 'out', fixture, variant);
      if (!EXTERNAL[variant] && !existsSync(root)) throw new Error(`missing ${root}; run node bench/fixtures.mjs`);
      // a phext variant whose bundle is byte-identical to an earlier one would measure the same thing twice
      const hash = bundleHash(fixture, variant);
      const twin = hash && VARIANTS.slice(0, vi).find((v) => bundleHash(fixture, v) === hash);
      if (twin) {
        for (const rtt of RTTS) {
          const t = results.find((r) => r.fixture === fixture && r.proto === proto && r.rtt === rtt && r.variant === twin);
          results.push({ ...t, variant, sameAs: twin });
          console.log(`${fixture.padEnd(8)} ${proto} rtt=${String(rtt).padStart(3)} ${variant.padEnd(11)} same bundle as ${twin}`);
        }
        continue;
      }
      // Link: preload goes to every subresource for `preload`, and to the bundle's externals for phext and inline
      const linksPath = EXTERNAL[variant]
        ? null
        : variant === 'preload'
          ? join(here, 'out', fixture, 'preload.links')
          : isPhext(variant)
            ? join(root, 'app.links')
            : variant === 'inline'
              ? join(here, 'out', fixture, 'phext', 'app.links')
              : null;
      const links = linksPath ? readFileSync(linksPath, 'utf8').split('\n').filter(Boolean) : [];
      const server = EXTERNAL[variant] ? externalServer(variant, proto) : await startServer({ root, proto, links, tls });
      for (const rtt of RTTS) {
        const proxy = await startProxy({ targetPort: server.port, rttMs: rtt, mbps: MBPS });
        const url = `https://127.0.0.1:${proxy.port}`;
        const warmDir = join(profiles, `${fixture}-${proto}-${variant}-${rtt}-warm`);
        if (isPhext(variant)) await warm(url, warmDir);
        const runs = [];
        for (let i = 0; i < RUNS; i++) {
          const dir = join(profiles, `run-${i}`);
          rmSync(dir, { recursive: true, force: true });
          if (isPhext(variant)) cpSync(warmDir, dir, { recursive: true });
          proxy.reset();
          server.reset();
          const m = await once(url, dir);
          runs.push({ ...m, connections: proxy.stats.connections, bytesDown: proxy.stats.bytesDown, requests: server.stats.requests, encoding: server.stats.bundleEncoding });
          rmSync(dir, { recursive: true, force: true });
        }
        await proxy.close();
        rmSync(warmDir, { recursive: true, force: true });
        const bad = runs.filter((r) => !r.ok || (isPhext(variant) && !r.sw));
        const row = {
          fixture,
          proto,
          variant,
          rtt,
          doneMedian: median(runs.map((r) => r.done)),
          doneP90: pct(runs.map((r) => r.done), 0.9),
          fcpMedian: median(runs.map((r) => r.fcp ?? NaN)),
          connections: median(runs.map((r) => r.connections)),
          requests: median(runs.map((r) => r.requests)),
          kbDown: median(runs.map((r) => r.bytesDown)) / 1024,
          invalid: bad.length,
          encoding: runs[0].encoding,
          runs,
        };
        results.push(row);
        console.log(
          `${fixture.padEnd(8)} ${proto} rtt=${String(rtt).padStart(3)} ${variant.padEnd(11)} done ${row.doneMedian.toFixed(0).padStart(5)} ms ` +
            `(p90 ${row.doneP90.toFixed(0)}) conns ${row.connections} reqs ${row.requests} ${row.kbDown.toFixed(1)} KB${row.encoding ? ` ${row.encoding}` : ''}${bad.length ? `  INVALID x${bad.length}` : ''}`,
        );
      }
      await server.close();
    }
  }
}
rmSync(profiles, { recursive: true, force: true });

const stamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
const outDir = join(here, 'results');
mkdirSync(outDir, { recursive: true });
const intro =
  `Chrome headless via a modeled path: RTT as listed, ${MBPS} Mbit/s, initcwnd 10 segments, TLS 1.3, cold HTTP cache, ` +
  `new connection per run; phext runs use a pre-installed Service Worker. ${RUNS} runs per cell; times are medians ` +
  `from navigation start to the page's done mark (scripts run, images decoded, data loaded).\n`;
const find = (f, p, rtt, v) => results.find((r) => r.fixture === f && r.proto === p && r.rtt === rtt && r.variant === v);
const rel = (a, b) => (a !== undefined && Number.isFinite(b) ? `${(((a - b) / b) * 100).toFixed(0)}%` : '');

// Pivot: one row per protocol x RTT, one column per variant; fastest in bold
let md =
  `# phext delivery benchmark — ${stamp}\n\n${intro}\nDone time in ms (median). **Bold** = fastest in the row. ` +
  `"phext vs best baseline" compares the default \`phext\` variant with the fastest of the non-phext variants on the same protocol.\n`;
const baselines = VARIANTS.filter((v) => !isPhext(v));
for (const fixture of FIXTURES) {
  md += `\n## ${fixture}\n\n| proto | RTT | ${VARIANTS.join(' | ')} | phext vs best baseline | phext vs h2 files |\n`;
  md += `|---|---|${VARIANTS.map(() => '---|').join('')}---|---|\n`;
  for (const rtt of RTTS) {
    for (const proto of PROTOS) {
      const rows = VARIANTS.map((v) => find(fixture, proto, rtt, v));
      const best = Math.min(...rows.map((r) => r.doneMedian));
      const cells = rows.map((r) => {
        const t = `${r.doneMedian.toFixed(0)}${r.invalid ? ` ⚠${r.invalid}` : ''}${r.sameAs ? '*' : ''}`;
        return r.doneMedian === best ? `**${t}**` : t;
      });
      const ph = find(fixture, proto, rtt, 'phext');
      const bestBase = Math.min(...baselines.map((v) => find(fixture, proto, rtt, v)?.doneMedian ?? Infinity));
      const h2files = find(fixture, 'h2', rtt, 'files');
      md += `| ${proto} | ${rtt} | ${cells.join(' | ')} | ${rel(ph?.doneMedian, bestBase)} | ${rel(ph?.doneMedian, h2files?.doneMedian)} |\n`;
    }
  }
}
md += `\n\\* same bundle bytes as an earlier phext variant, so its result is reused rather than re-measured.\n`;

// Detail: every metric per cell
let detail = `# phext delivery benchmark — ${stamp} (detail)\n\n${intro}`;
for (const fixture of FIXTURES) {
  detail += `\n## ${fixture}\n\n| proto | RTT | variant | done (ms) | p90 | FCP | conns | requests | KB down |\n|---|---|---|---|---|---|---|---|---|\n`;
  for (const rtt of RTTS) {
    for (const proto of PROTOS) {
      for (const v of VARIANTS) {
        const r = find(fixture, proto, rtt, v);
        detail +=
          `| ${proto} | ${rtt} | ${v}${r.sameAs ? ` (= ${r.sameAs})` : ''}${r.invalid ? ` ⚠${r.invalid}` : ''} | ` +
          `${r.doneMedian.toFixed(0)} | ${r.doneP90.toFixed(0)} | ${r.fcpMedian.toFixed(0)} | ${r.connections} | ${r.requests} | ${r.kbDown.toFixed(1)} |\n`;
      }
    }
  }
}
writeFileSync(join(outDir, `${stamp}.json`), JSON.stringify({ config: { FIXTURES, PROTOS, VARIANTS, RTTS, RUNS, MBPS }, results }, null, 2));
writeFileSync(join(outDir, `${stamp}.md`), md);
writeFileSync(join(outDir, `${stamp}-detail.md`), detail);
writeFileSync(join(outDir, 'latest.md'), md);
console.log(`\nwrote bench/results/${stamp}.md and ${stamp}-detail.md`);
