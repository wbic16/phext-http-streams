// Checks a Node phext implementation against core/test-vectors.
// Usage: node check_node.mjs [path-to-libphext.js]   (default: ../../../libphext.node/dist/libphext.js)
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const lib = resolve(process.argv[2] ?? join(here, '../../../libphext.node/dist/libphext.js'));
const { Phext } = await import(pathToFileURL(lib).href);

const fmt = (c) => `${c.z.library}.${c.z.shelf}.${c.z.series}/${c.y.collection}.${c.y.volume}.${c.y.book}/${c.x.chapter}.${c.x.section}.${c.x.scroll}`;

let failures = 0;
const vectors = readdirSync(here).filter((f) => f.endsWith('.phext')).sort();
for (const file of vectors) {
  const name = file.slice(0, -'.phext'.length);
  const text = readFileSync(join(here, file), 'utf8');
  const want = JSON.parse(readFileSync(join(here, `${name}.expected.json`), 'utf8'));
  const got = new Phext().phokenize(text).filter((p) => p.scroll.length > 0).map((p) => ({ coord: fmt(p.coord), text: p.scroll }));
  if (JSON.stringify(got) !== JSON.stringify(want)) {
    failures++;
    let i = 0;
    while (i < got.length && i < want.length && JSON.stringify(got[i]) === JSON.stringify(want[i])) i++;
    console.log(`FAIL ${name}: ${got.length} scrolls vs ${want.length} expected; first difference at #${i}`);
    console.log('  got: ', JSON.stringify(got[i] ?? null).slice(0, 200));
    console.log('  want:', JSON.stringify(want[i] ?? null).slice(0, 200));
  }
}
console.log(`node (${lib}): ${vectors.length - failures}/${vectors.length} vectors match`);
process.exit(failures ? 1 : 0);
