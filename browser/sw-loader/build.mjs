// Builds dist/phext-sw.js (Service Worker with the WASM reader embedded) and dist/__phext-warm.html.
// Usage: node browser/sw-loader/build.mjs   (runs the cargo wasm build first)
import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync, copyFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '../..');
execFileSync('cargo', ['build', '--quiet', '--profile', 'wasm', '--target', 'wasm32-unknown-unknown', '-p', 'phext-stream-wasm'], {
  cwd: root,
  stdio: 'inherit',
});
const wasm = readFileSync(join(root, 'target/wasm32-unknown-unknown/wasm/phext_stream_wasm.wasm'));
// replace the quoted constant only (the header comment mentions the placeholder too)
const template = readFileSync(join(here, 'sw.template.js'), 'utf8');
if (!template.includes("'__WASM_BASE64__'")) throw new Error('placeholder missing from sw.template.js');
const sw = template.replace("'__WASM_BASE64__'", () => `'${wasm.toString('base64')}'`);
mkdirSync(join(here, 'dist'), { recursive: true });
writeFileSync(join(here, 'dist/phext-sw.js'), sw);
copyFileSync(join(here, 'warm.html'), join(here, 'dist/__phext-warm.html'));
console.log(`dist/phext-sw.js: ${sw.length} bytes (wasm ${wasm.length})`);
