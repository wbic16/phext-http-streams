# Benchmark results

**Shipped (2026-10-26): `phext-bundle` inlines everything by default.** In the sections below, which predate
that change, `phext-all` is the shipped default (byte-identical bundles) and `phext` is today's `phext-2rt`.

## Phase 2: nginx and httpd modules (2026-10-26)

### Chrome end-to-end through real nginx

[results/nginx/](results/nginx/): 6 runs per cell, no invalid runs. `phext-nginx` = nginx 1.18 + `ngx_http_phext_module` building
the bundle from the fixture's files. `phext-nginx-static` = the same nginx serving the pre-built bundle via
`gzip_static`. `phext` = the Node origin serving the pre-built bundle. Done time, ms:

| Fixture | Proto | RTT | `phext` (Node) | `phext-nginx-static` | **`phext-nginx` (module)** |
|---|---|---|---|---|---|
| small | h2 | 50 | 259 | 257 | **257** |
| small | h1 | 200 | 1038 | 1012 | **1013** |
| spa | h2 | 200 | 1215 | 1216 | **1217** |
| spa | h1 | 200 | 1221 | 1220 | **1221** |
| icons | h2 | 50 | 260 | 261 | **262** |
| channels | h1 | 200 | 833 | 834 | **835** |

In all 16 cells, the module is within 3 ms of nginx serving the same bundle as a static file. The one
exception is small/h2/200 ms, at +24 ms with a noisy p90. It's also within a few ms of the Node origin. The
module adds no measurable page-load latency, and the HTTP/1.1 results from Phase 1 carry over to a
production server.

**What an earlier run caught.** The first end-to-end run served fixtures from `/mnt/c`, a 9P share under WSL,
and the module was 70–400 ms slower. It re-checked the directory on the request path, and one stat walk of
the 102-file `spa` directory takes **607 ms on `/mnt/c` versus 0.15 ms on ext4**. Both modules now re-check
off the request path: an nginx per-worker timer, and an httpd watcher thread. The benchmark serves from the
local filesystem, as a deployment would.

### Load ([results/load.md](results/load.md))

`h2load`, 20,000 requests, 16 connections, `spa` bundle (394 KB plain), WSL2 on 16 CPUs:

| | req/s | 2xx |
|---|---|---|
| nginx HTTP/2, module | 11,839 | 20,000 |
| nginx HTTP/2, `gzip_static` pre-built | 11,080 | 20,000 |
| nginx HTTP/1.1 (TLS), module | 11,081 | 20,000 |
| nginx HTTP/1.1 (TLS), `gzip_static` | 10,352 | 20,000 |
| httpd HTTP/1.1, mod_phext | 12,616 | 20,000 |

The module serves at the rate nginx serves a static file. Rows vary ±20% between runs on this machine, so
read them as parity, not as one beating the other. A full build of this bundle (walk, assemble, gzip 6 +
br 5 + zstd 3) takes **13 ms per nginx worker** (at startup) and **17–18 ms per httpd child** (on its
first request).

**What the load test caught.** mod_phext initially returned 500 to requests that arrived while a new child
process was still building its first bundle (524 of 20,000). Those requests now wait for the build.
`servers/test.sh` covers this with 64 concurrent first requests per server.

## Variation matrix (2026-10-26): budget, gzip flushing, encoders

Full tables: [results/2026-10-26T03-31-45.md](results/2026-10-26T03-31-45.md) (pivot),
[detail](results/2026-10-26T03-31-45-detail.md), raw runs in the `.json`. Rechecks with 12 runs per cell are in
[results/recheck/](results/recheck/). Encoder sizes and speeds: [results/codecs.md](results/codecs.md).

Setup: as below, plus RTT 20 / 50 / 100 / 200 ms, 4 fixtures (`icons` added: 24 small images plus a hero),
6 runs per cell, 352 cells, no invalid runs. The default `phext` now uses the budget of HTTP/2's
request → response → request window after its handshake: 2 slow-start rounds = 43,800 bytes. It also
uses one-shot gzip from libdeflate -12.

### 0. Text in phext, binaries left to HTTP/2 multiplexing (`phext-text`)? Not optimal for load time.

Quick test, [results/2026-10-26T04-13-50.md](results/2026-10-26T04-13-50.md): 8 runs per cell, no invalid runs.
`phext-text` puts every text asset in the bundle and every binary outside it (`--inline-max 0`). On `spa` it
is byte-identical to the default, which already leaves all images out. Done time / first paint (FCP), in ms:

| Fixture | Proto | RTT | `phext` (2-round budget) | `phext-text` | `phext-all` | best baseline |
|---|---|---|---|---|---|---|
| small | h2 | 50 | 315 / 184 | 312 / **180** | **257** / 232 | 311 (inline) |
| small | h2 | 200 | 1219 / 628 | 1215 / 632 | **1007** / 832 | 1437 (inline) |
| icons | h2 | 50 | 317 / 232 | 320 / **180** | **262** / 236 | 325 (preload) |
| icons | h2 | 200 | 1220 / 838 | 1252 / **666** | **1013** / 840 | 1412 (inline) |
| small | h1 | 200 | 1214 / 632 | 1619 / 632 | **1014** / 844 | 1343 (inline) |
| icons | h1 | 200 | 1223 / 842 | 2237 / **638** | **1015** / 840 | 1355 (inline) |

- **Over HTTP/2, text-only ties the 2-round default on done time, but inlining everything is still about
  one RTT (~17%) faster.** The multiplexed image requests can't start until the bundle's header scroll
  arrives (the Service Worker prefetches them then), and that request round trip costs more than carrying
  the bytes in the already-flowing bundle.
- **Text-only gives the earliest first paint.** The bundle is small (2.7 KB for `icons`), so the document
  and CSS land a round trip sooner: 666 vs 840 ms at 200 ms RTT.
- **Over HTTP/1.1, text-only is clearly worse.** The images go back to 6 connections, each with its own handshake:
  `icons` takes 2237 vs 1015 ms at 200 ms RTT.
- On `spa` over HTTP/2 all three variants tie (315 / 1214 ms): its text alone already needs 3 rounds, and the
  images fit alongside it.

So it depends on the goal: inline everything for the fastest complete page; text-only for the earliest
first paint and separately cacheable images, if images can start without waiting on the header. A
Service Worker could do that by remembering the previous visit's header and fetching the externals in
parallel with the bundle (untested).

### 1. One phext response makes HTTP/1.1 as fast as HTTP/2

With everything in the bundle (`phext-all`), HTTP/1.1 lands within 4% of HTTP/2 in all 16 fixture × RTT
rows (median +1%). It is also 27–56% faster than HTTP/2 serving plain files. At 200 ms RTT:

| Fixture | HTTP/1.1 files | HTTP/2 files | HTTP/2 best baseline | **HTTP/1.1 phext-all** | HTTP/2 phext-all |
|---|---|---|---|---|---|
| small | 1798 | 1848 | 1390 (inline) | **1013** | 1008 |
| spa | 3245 | 2217 | 1369 (inline) | **1218** | 1216 |
| channels | 3429 | 1695 | 989 (inline) | **830** | 829 |
| icons | 2409 | 1571 | 1421 (preload) | **1016** | 1010 |

The default `phext` leaves large images external. Those still go through HTTP/1.1's 6-connection pool, which
is why the default `phext` over HTTP/1.1 trails HTTP/2 on `spa` (2032 vs 1217 ms at 200 ms) and
`phext-all` doesn't.

### 2. The budget: bigger was better in every case tested

| | `phext-1rt` (14.6 KB) | `phext` (2 rounds, 43.8 KB) | `phext-3rt` (102 KB) | `phext-all` |
|---|---|---|---|---|
| beats best baseline (of 32 rows) | — | 25 | — | **28** |
| icons, HTTP/1.1, 200 ms | 1926 | 1225 | 1225 | **1016** |
| spa, HTTP/1.1, 200 ms | 2032 | 2032 | 1418 | **1218** |
| small, HTTP/2, 200 ms | 1223 | 1223 | 1223 | **1008** |

Why: an external resource can't be requested until the header scroll arrives, so it costs at least one
extra round trip plus its own slow start on that request. Bytes added to the bundle ride a congestion
window that is already growing, at most one extra round per doubling, and those rounds overlap the ones
already being paid. The 1-round budget is clearly worst: on `icons` it pushes 16 images out of the bundle.

`phext-all` wins on `done` everywhere tested. Before making it the default, weigh what this model doesn't
capture:
- no loss and 50 Mbit/s, so large inline bytes are cheap here;
- the cache value of large, rarely-changing assets;
- first paint: `phext-all` delays FCP by about one RTT on `small` (836 vs 640 ms over HTTP/2 at 200 ms). It doesn't on
  `spa` or `icons`. The cause isn't compression (the flushed `phext-all-flush` behaves identically) and is
  still unexplained.

### 3. One-shot vs per-scroll gzip flush: no difference

Across all 32 rows, `phext-flush` is within noise of the one-shot default. The rows that looked different on
6 runs (for example icons HTTP/2 50 ms, 439 vs 315 ms) re-measure at 317 vs 316 ms with 12 runs. That
holds for the 177 KB all-inline SPA bundle too. One-shot stays the default.

### 4. Encoders: smaller output, same load time at these sizes

From `codecs.md`, on these bundles:

| Encoder | Size vs gzip -9 (text-heavy) | Compress MB/s | Notes |
|---|---|---|---|
| libdeflate -12 (new default) | −9 to −10% | 2–10 | still plain gzip, every client |
| zopfli i15 | −10 to −13% | 0.1 | 19–96× slower than libdeflate -12, for 1–3% smaller output |
| brotli q11 | −14 to −15% | 0.4–1.3 | `Content-Encoding: br` |
| zstd -19 | −8 to −14% | 2–7 | `Content-Encoding: zstd` |
| zstd -3 | −2 to +10% | 220–400 | fastest for per-request bundling |
| libdeflate -6 | −6 to +5% | 25–100 | fastest gzip for per-request bundling |

No encoder changed any fixture's slow-start round count, and in Chrome `phext-flate2`, `phext-br`, and
`phext-zstd` all match the default within noise. Again, the outliers re-measure equal at 12 runs: icons HTTP/1.1
20 ms is 140 vs 141 ms, and small HTTP/1.1 50 ms with br is 317 vs 317 ms. Encoder choice matters for
bandwidth-limited links and for the server's CPU when bundling per request (Phase 2: zstd -3 where
accepted, else libdeflate -6), not for latency here.

### 5. Where phext still loses

`channels` at 20–50 ms RTT: the inline page is 20 ms faster (89 vs 111 ms, 210 vs 229 ms). With so little
RTT to save, the Service Worker hop and WASM decode are the cost. Native support (plan Stage B) is the fix.

### Noise

Slow outliers (more than 25% over the cell median) are about as common in baseline runs (5.6%) as in phext
runs (4.3%). They move the same bytes over the same connections as fast runs, so they come from the host
(Chrome, the worker, and the proxy's spin scheduler share one machine), not from the modeled network. Cells
that matter for a conclusion were re-measured with 12 runs.

---

## Phase 1 results (2026-10-26, first run)

Full tables: [results/2026-10-26T00-04-17.md](results/2026-10-26T00-04-17.md) (raw runs in the `.json` beside it).
That run used a 14 KB budget and per-scroll gzip flushing (since replaced by the defaults above).
Setup: headless Chrome, modeled network (RTT 50 / 150 ms, 50 Mbit/s, initcwnd 10, TLS 1.3, no loss), cold HTTP
cache and new connections every run, 8 runs per cell, medians. phext runs use an already-installed Service
Worker (returning visitor). See [README.md](README.md) for the model and method.

## Headline: phext gives HTTP/1.1 the multiplexing it lacks

Time to "done" (scripts run, images decoded, data loaded), in ms:

| Fixture | RTT | HTTP/1.1 files | HTTP/2 files | **HTTP/1.1 phext** | HTTP/2 phext |
|---|---|---|---|---|---|
| small (15 requests) | 50 | 427 | 528 | **313** | 311 |
| small | 150 | 1457 | 1410 | **920** | 918 |
| spa (57 requests) | 50 | 858 | 659 | **522** | 317 |
| spa | 150 | 2486 | 1686 | **1524** | 916 |
| channels (55 requests) | 50 | 887 | 479 | **320** | 226 |
| channels | 150 | 2588 | 1282 | **632** | 628 |

- **HTTP/1.1 + phext beats HTTP/2 without phext in every cell**: 10% to 51% faster than HTTP/2 files.
- **When everything is in the bundle, the protocol stops mattering.** For `small` and `channels` at 150 ms,
  HTTP/1.1 phext and HTTP/2 phext are within 4 ms of each other. One bundle on one connection is the multiplexing.
- **Where HTTP/1.1 phext still trails HTTP/2 phext (`spa`), it's the externals.** The SPA's text alone is
  ~54 KB gzip'd, over the 14 KB flight budget, so its 10 icons and 2 images stay external. Those go out over
  HTTP/1.1's 6-connection pool (6 connections, 15 requests). That is the inline rule working as designed;
  raising the budget or inlining icons is a tuning question (see "Next").
- HTTP/1.1 without phext is the expected disaster for many small resources: `channels` takes 2.6 s at
  150 ms, 2× HTTP/2.

## Against the best HTTP/2 baselines

| Fixture | RTT | h2 preload + 103 | h2 inline-everything | **h2 phext** |
|---|---|---|---|---|
| small | 50 | 419 | 409 | **311** |
| small | 150 | 1041 | 1069 | **918** |
| spa | 50 | 383 | 326 | **317** |
| spa | 150 | 1262 | 1093 | **916** |
| channels | 50 | 270 | **209** | 226 |
| channels | 150 | 958 | 759 | **628** |

- At 150 ms phext beats both Early Hints and the inline-everything page in every fixture (12–34% vs
  preload, 14–17% vs inline).
- At 50 ms it ties or wins, except `channels`, where the inline page is 17 ms faster. With little RTT to
  save, the Service Worker hop and WASM decode cost about as much as they win.
- phext's first contentful paint is the earliest or tied in most cells, because the document scroll streams
  to the parser before the rest of the bundle has arrived.
- Bytes on the wire: phext and inline are the lowest (one gzip context across all text). The bundle is
  ~2% smaller than the inline page for `small`, and they're about equal elsewhere.

## Decision gate (plan §6): go

phext beats every baseline on every fixture at 150 ms, over both protocols. At 50 ms over HTTP/2 it is
within 17 ms of the inline page at worst. Over HTTP/1.1 at 50 ms it ties inline on `small`, trails it by
10 ms on `spa`, and trails it by 112 ms on `channels` (the unexplained cell below). It also does what
neither Early Hints nor inlining can:
- it keeps resources separately addressable and cacheable (per-scroll xxh3 manifests, delta sync later);
- it fetches the large binaries it leaves out in parallel, as soon as the header scroll arrives;
- it removes HTTP/1.1's head-of-line and connection-limit penalty for everything in the bundle.

## Caveats

- **Returning visitors only.** phext ran with the Service Worker already installed. A first visit pays for
  installing it and does not benefit; native browser support (plan Stage B) is what removes that.
  First-visit cost is not measured yet.
- Synthetic sites, a modeled network (no loss, no slow-start-after-idle), one machine, 8 runs per cell. Absolute
  numbers are model-specific; the relative ordering is the result.
- One unexplained cell: `channels` HTTP/1.1 phext at 50 ms (320 ms, p90 354) is ~95 ms slower than the same
  bundle over HTTP/2 (226 ms), while at 150 ms the two are equal. Worth tracing before quoting the 50 ms
  HTTP/1.1 number.
- The `inline` variant uses the bundler's inline/external split, so it is a strong, fair baseline, not a
  strawman. It is still not cacheable per resource, which these cold-cache runs don't show.

## Next

1. Trace the `channels` h1 @ 50 ms outlier (Chrome net-log on one run).
2. Measure the first visit (Service Worker install included) and a repeat visit with a warm HTTP cache.
3. Sweep `--flight-budget` (14 / 28 / 42 KB) and the flush policy, especially for `spa` over HTTP/1.1.
4. Phase 2: nginx/httpd modules for dynamic bundling, using the same core through the C ABI.
