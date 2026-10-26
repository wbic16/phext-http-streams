// One-off: load a fixture variant once (no proxy) and print every request the origin saw.
// Usage: node bench/debug-requests.mjs <fixture> <variant> <h1|h2> [links-file]
import { createHash, X509Certificate } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import puppeteer from 'puppeteer-core';
import { startServer } from './server.mjs';
const here = dirname(fileURLToPath(import.meta.url));
const [fixture = 'small', variant = 'preload', proto = 'h2', linksName] = process.argv.slice(2);
const pem = readFileSync(join(here, 'certs/cert.pem'));
const spki = createHash('sha256').update(new X509Certificate(pem).publicKey.export({ type: 'spki', format: 'der' })).digest('base64');
const links = linksName ? readFileSync(join(here, 'out', fixture, linksName), 'utf8').split('\n').filter(Boolean) : [];
const server = await startServer({ root: join(here, 'out', fixture, variant), proto, links, tls: { key: readFileSync(join(here, 'certs/key.pem')), cert: pem } });
const browser = await puppeteer.launch({ executablePath: 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true, userDataDir: join(here, 'profiles/debug-req'), args: [`--ignore-certificate-errors-spki-list=${spki}`, '--no-first-run'] });
const page = await browser.newPage();
await (await page.createCDPSession()).send('Network.clearBrowserCache');
await page.goto(`https://127.0.0.1:${server.port}/`, { waitUntil: 'load' });
await page.waitForFunction(() => window.__done !== undefined, { timeout: 15000 });
const counts = {};
for (const p of server.stats.paths) counts[p] = (counts[p] || 0) + 1;
console.log(`${server.stats.requests} requests; repeated:`, Object.entries(counts).filter(([, n]) => n > 1));
await browser.close();
await server.close();
