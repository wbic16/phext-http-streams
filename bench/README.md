# Delivery benchmark

Compares four ways of delivering the same site, over HTTP/1.1 and HTTP/2, in headless Chrome:

| Variant | What the browser gets |
|---|---|
| `files` | Plain files, gzip'd. The browser discovers each one (HTML → CSS/JS → imports → images/data) and fetches it. |
| `preload` | Same files, plus `Link: preload` / `modulepreload` for every subresource on the document (and `103 Early Hints`, which Chrome honors over HTTP/2 only). |
| `inline` | One HTML document with all CSS, JS, data, and images inlined, mirroring the default bundle (anything it leaves external stays separate here too). The strongest "one response" baseline. |
| `phext` | Same files plus `app.phext.gz` from `phext-bundle` with its shipped defaults: everything inline (binaries as base64), one-shot gzip by libdeflate -12. Served through the Service Worker loader (`browser/sw-loader`). One request carries the whole page. |
| `phext-1rt` / `phext-2rt` / `phext-3rt` | Budgeted mode: binaries up to 8 KB inline while the gzip'd bundle fits 1 / 2 / 3 slow-start rounds (14.6 / 43.8 / 102.2 KB); the rest external. `phext-2rt` was the default before inline-everything shipped. |
| `phext-text` | Text only; every binary left external for HTTP/2 (`--inline-max 0`). |
| `phext-flush` | Everything inline, flate2 with a sync-flush after every scroll. |
| `phext-flate2` | Everything inline, one-shot flate2 -9 instead of libdeflate -12. |
| `phext-br` / `phext-zstd` | Everything inline, served as `Content-Encoding: br` (q11) / `zstd` (-19). |
| `phext-nginx` / `phext-nginx-static` | Real nginx: the bundle built on the fly by `ngx_http_phext_module`, or the pre-built bundle via `gzip_static`. Start it first with `servers/nginx-bench.sh start <fixture>` (Linux/WSL); see [../servers/README.md](../servers/README.md). |

Results before inline-everything shipped (2026-10-26, in `RESULTS.md`) used the earlier names: there `phext` is
today's `phext-2rt`, `phext-all` is today's `phext` (byte-identical bundles), and the encoder and flush
variants used the 2-round budget.

Fixtures: `small` (landing page, 15 requests), `spa` (module graph three levels deep, 57 requests),
`channels` (50 JSON channels fetched in parallel), `icons` (24 small images plus a hero). A phext variant
whose encoded bundles are byte-identical to an earlier variant's reuses that variant's measurement (marked
`*` in the summary).

Encoder sizes and speeds on these bundles: `cargo run --release -p phext-bundle --example codecs`
([results/codecs.md](results/codecs.md)).

## Running

```bash
cd bench && npm install
```

```bash
node bench/fixtures.mjs
```

```bash
node bench/run.mjs --fixtures small,spa,channels --protos h1,h2 --rtt 50,150 --runs 8
```

Results land in `bench/results/<timestamp>.md` (and `.json` with every run). `node bench/test-proxy.mjs`
checks the network model; `node bench/debug-sw.mjs [fixture]` opens a phext fixture with console output;
`node bench/debug-requests.mjs <fixture> <variant> <h1|h2> [links-file]` lists the requests one load makes.

Latest findings: [RESULTS.md](RESULTS.md).

## Network model (`proxy.mjs`)

Chrome connects to a TCP proxy in front of the origin (`server.mjs`, Node `https`/`http2` with TLS 1.3):

- fixed RTT, half each way; a TCP handshake costs one RTT before the client's first byte leaves;
- downstream slow start per connection: `initcwnd` = 10 × 1460 bytes, doubling after each window-limited
  round trip;
- a downstream bottleneck (default 50 Mbit/s).

Not modeled: loss, slow-start-after-idle, receive windows, and the upstream bottleneck. Each HTTP/1.1
connection gets its own congestion window, as in real TCP, which is what lets HTTP/1.1's 6 parallel
connections claw back some throughput. On Windows the default timer tick is ~15.6 ms, so the proxy uses a
spin-assisted scheduler; `test-proxy.mjs` checks handshake, slow-start rounds, and bandwidth timing to
within a few ms.

## Method

- A new Chrome process per run, so every run pays DNS-free connection setup (TCP + TLS) through the proxy.
- HTTP cache cleared before navigating.
- `phext` runs use a profile with the Service Worker already installed (a returning visitor). A first visit
  pays the worker's install before it can help. Native browser support (plan Stage B) removes that cost.
- The page's done mark (`window.__done`) is set once its module graph has run, its images have decoded, and
  its data has loaded. Runs where that didn't happen (or where the worker wasn't in control for `phext`) are
  flagged as invalid in the summary.
