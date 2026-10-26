// One-off: open the warm page and the app through the SW, printing console output and errors.
import { createHash, X509Certificate } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import puppeteer from 'puppeteer-core';
import { startServer } from './server.mjs';
const here = dirname(fileURLToPath(import.meta.url));
const pem = readFileSync(join(here, 'certs/cert.pem'));
const spki = createHash('sha256').update(new X509Certificate(pem).publicKey.export({ type: 'spki', format: 'der' })).digest('base64');
const fixture = process.argv[2] || 'small';
const server = await startServer({ root: join(here, 'out', fixture, 'phext'), proto: 'h2', tls: { key: readFileSync(join(here, 'certs/key.pem')), cert: pem } });
const browser = await puppeteer.launch({ executablePath: 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true, userDataDir: join(here, 'profiles/debug'), args: [`--ignore-certificate-errors-spki-list=${spki}`, '--no-first-run'] });
const page = await browser.newPage();
page.on('console', (m) => console.log('[page]', m.text()));
page.on('pageerror', (e) => console.log('[pageerror]', e.message));
const url = `https://127.0.0.1:${server.port}`;
await page.goto(`${url}/__phext-warm.html`, { waitUntil: 'load' });
await page.waitForFunction(() => document.title !== 'phext: installing', { timeout: 15000 }).catch(() => {});
console.log('warm:', await page.title(), '|', await page.$eval('#s', (e) => e.textContent));
if ((await page.title()) === 'phext: ready') {
  await page.goto(`${url}/`, { waitUntil: 'load' });
  await page.waitForFunction(() => window.__done !== undefined, { timeout: 15000 }).catch(() => console.log('no __done'));
  console.log(await page.evaluate(() => ({ done: window.__done, ok: window.__ok, sw: !!navigator.serviceWorker.controller })));
  console.log('server saw:', server.stats.paths.join(' '));
}
await browser.close();
await server.close();
