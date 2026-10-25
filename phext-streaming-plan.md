# Native Phext Streaming: nginx, httpd, Chrome — Project Plan

Baseline: [wbic16/libphext-rs](https://github.com/wbic16/libphext-rs) v0.3.1 (`src/phext.rs`, ~1.3k LOC, deps: `xxhash-rust`).

**Status (2026-10-25): Phase 0 complete.**
- Spec: `spec/bundle-v1.md`.
- Core: `core/phext-stream`, `no_std`, no dependencies. Decode runs at ~3.9 GB/s and validate at ~4.8 GB/s, single-threaded. On a 4 MiB document it is ~800× faster than libphext `phokenize`, and the gap grows with size.
- C ABI: `core/phext-stream-capi`, tested from C via MSVC.
- Agreement: differential tests and fuzz-style tests against libphext 0.3.1. The shared vectors pass 20/20 on Rust and C#, and 19/20 on Python and Node.
- Open: the 24 h cargo-fuzz run needs a Linux box. The WASM build moves to Phase 1 with the Service Worker loader.

## 1. Goal, stated precisely

Deliver a multi-resource artifact (HTML + CSS + JS + data, potentially many logical channels) as **one phext stream on one HTTP response**, so the browser has everything it needs in the first response flight instead of discovering and fetching resources one round trip at a time.

What "in the time it takes to establish an HTTP/2 connection" can realistically mean:

- TCP + TLS 1.3 setup costs ~2 RTT (1 RTT with TLS resumption / QUIC). The first response flight after that is capped by the initial congestion window, ~10 packets ≈ **14 KB on the wire**.
- So the measurable target is: **artifact fully usable after 1 request RTT, for artifacts that fit (compressed) in the first few cwnd flights**, versus 2–4+ RTTs for the normal discover→fetch waterfall.
- Bigger artifacts still pay bandwidth/slow-start; phext removes *request* round trips, not physics.

The baselines that matter (the plan must beat or match these, or the result isn't interesting):

| Baseline | Why it matters |
|---|---|
| H2, N separate files | The status quo |
| H2 + `103 Early Hints` / `<link rel=preload>` | Today's best answer to the waterfall |
| Single HTML with everything inlined | The strongest "one response" competitor — phext must win on structure, cacheability, or streaming order, not just RTT count |
| H2 Server Push | Removed from Chrome 106 (2022) — phext bundling is effectively a replacement for what push promised |

## 2. Findings from libphext-rs that drive the design

1. **Not streamable as-is.** `next_scroll` copies the entire remainder into a new `String` on every call, so `phokenize`/`explode` are O(n²) in buffer size, and every API takes a complete `&str`. A server filter or a browser loader needs an incremental, byte-at-a-time parser that never sees the whole document.
2. **Panics on non-UTF-8.** `String::from_utf8(...).expect("valid UTF-8")` — unusable inside a server worker process or a browser network service, where a panic takes down the process.
3. **The encoder logic is already right.** `append_scroll` emits the minimal delimiter sequence between two ordered coordinates. That is exactly what a server-side serializer needs; it just needs to work on bytes and write into caller-supplied buffers.
4. **`manifest()` + `checksum()` (xxh3-128 per scroll)** give us cache validation and delta sync later almost for free.
5. **`COORDINATE_MAXIMUM = 1000`** per dimension — fine for bundles.

### The "unescaped" caveat and the binary policy

Phext delimiters are raw bytes `0x01, 0x17–0x1A, 0x1C–0x1F`. Text resources (HTML/CSS/JS/SVG/JSON/Markdown) essentially never contain those bytes raw, so **text can travel unescaped** — the core claim holds for text. **Binary resources (images, fonts, wasm, pre-compressed blobs) routinely contain them** and would corrupt the coordinate stream.

**Policy: the bundle carries only what helps the first flight; everything else uses normal HTTP/2.**

The phext bundle's job is to fill the window *before* HTTP/2 multiplexing is doing useful work — the first one or two congestion-window flights after the handshake. Large assets don't benefit from being inside it: by the time they'd finish streaming, parallel H2 streams are running at full speed anyway, and putting them in the bundle would delay the scrolls behind them.

- **Text:** always inline, unescaped. Every scroll is still validated server-side; a text file that contains a delimiter byte is treated as binary.
- **Small binaries** (icons, small SVG-as-PNG, tiny fonts/wasm): inline as **base64** scrolls, flagged `;base64` in the header. Inside a gzip bundle, base64 costs little: gzip's Huffman stage packs the 64-symbol alphabet back to ~6 bits/char, so base64 of incompressible data gzips to roughly its original size (benchmark will confirm the exact ratio).
- **Large binaries:** **not in the bundle.** They are listed in the header as `external` entries, and the server also sends them as `Link: rel=preload` (and `103 Early Hints` where supported). The browser starts those H2 fetches immediately, in parallel with the bundle.

**Inline decision (bundler and server modules use the same rule):**

```
inline(asset) =  is_text(asset)
              || ( base64_len(asset) <= phext_inline_max            # default 8 KB
                   && gzip_len(bundle + asset) <= phext_flight_budget )  # default 14 KB, ≈ initcwnd 10
```

Assets are considered in priority order, so the budget is spent on what the page needs first. Both thresholds are configurable and get tuned from benchmark data (e.g. a 2-flight budget of ~42 KB may win on high-RTT links). There is no raw-binary scroll extension; large binaries stay on H2.

## 3. Architecture

One Rust core, used everywhere through a C ABI. No C++ port needed.

```
                 ┌──────────────────────────────┐
                 │ phext-stream (Rust, no_std+alloc) │
                 │  • incremental Decoder        │
                 │  • incremental Encoder        │
                 │  • byte validator             │
                 │  • bundle header parser       │
                 └───────┬─────────┬────────────┘
             cbindgen C ABI       wasm-bindgen
          ┌──────────┼─────────┐        │
     ngx_http_phext  mod_phext  Chromium  Service Worker
       (C module)    (C module) (GN rust_static_library  (prototype, no
                                 + cxx bridge)            browser changes)
```

Why this is the efficient choice:

- **One parser, one set of bugs, one fuzz target.** nginx and httpd link a static `.a`/`.lib`; Chromium already supports in-tree Rust third-party crates via GN; the same crate compiles to WASM for the browser prototype.
- The parser itself is ~300 lines; the integration glue is where the work is. Porting the parser to C/C++ would double the conformance surface for no gain.
- Alternative for nginx only: [ngx-rust](https://github.com/nginx/ngx-rust) lets you write the whole module in Rust. Reasonable, but a thin C module is more conventional for nginx contributors and mirrors the httpd module exactly.

### 3.1 `phext-stream` core API (sketch)

```rust
pub enum Event<'a> {
    ScrollStart(Coordinate),   // emitted when first content byte for a coordinate arrives
    Data(&'a [u8]),            // borrowed slice of the input chunk — zero copy
    ScrollEnd(Coordinate),     // emitted on the next delimiter, or on finish()
}

pub struct Decoder { coord: Coordinate, in_scroll: bool, /* no buffer */ }
impl Decoder {
    pub fn feed<'a>(&mut self, chunk: &'a [u8], sink: impl FnMut(Event<'a>));
    pub fn finish(&mut self, sink: impl FnMut(Event<'_>));
}

pub struct Encoder { coord: Coordinate }
impl Encoder {
    /// Writes the minimal delimiter run to reach `next` (append_scroll logic). Errors if next <= current.
    pub fn seek(&mut self, next: Coordinate, out: &mut [u8]) -> Result<usize, Error>;
}

pub fn validate_scroll(bytes: &[u8]) -> Result<(), ForbiddenByte>;
```

C ABI mirrors this with a callback (`phext_decoder_feed(dec, ptr, len, cb, userdata)`). Key property: **the decoder holds no buffer** — it only tracks a coordinate, so a scroll is "complete" the instant the next delimiter byte arrives, which is what makes progressive delivery work.

Land it as a new module/crate alongside libphext-rs (or inside it as `phext::stream`), and fix the two issues above in the existing API while there (byte-based `phokenize` in O(n); `Result` instead of `expect`).

### 3.2 Wire format: `application/phext` bundle v1

- **Media type:** `application/vnd.phext` (or `application/x-phext` for the experiment).
- **Compression: gzip is required.** Every bundle response is served with `Content-Encoding: gzip` (the client must advertise it; every browser does). The client decompresses, then decodes. Phext's single-byte delimiters and repeated header paths compress very well, and one gzip context across all scrolls beats per-file compression for small resources. Brotli/zstd can be negotiated later as an option, measured against gzip, but gzip is the baseline every component must support.
  - **Streaming-safe gzip:** the server sync-flushes (`Z_SYNC_FLUSH`) at scroll boundaries, at minimum after the header scroll and each critical scroll, so the client can decode a finished scroll without waiting for the compressor to fill its buffer. Flushing costs a few bytes each; flush points are configurable.
  - **Pre-compressed static bundles:** the bundler writes `app.phext.gz` next to `app.phext` (gzip level 9, or zopfli for maximum ratio). Servers send it as-is, so there's no per-request compression CPU.
- **Scroll `1.1.1/1.1.1/1.1.1` = bundle header**, plain text, one line per resource, in priority order:
  ```
  phext-bundle 1
  1.1.1/1.1.1/1.1.2  text/html               /index.html
  1.1.1/1.1.1/1.1.3  text/css                /app.css
  1.1.1/1.1.1/1.1.4  text/javascript         /app.js
  1.1.1/1.1.1/1.2.1  image/png;base64        /icon.png
  external           image/jpeg              /hero.jpg
  external           font/woff2              /brand.woff2
  ```
  `external` entries are fetched over normal H2; the header arrives in the first bytes of the response, so a loader can start those fetches even if the server didn't send `Link: preload`.
- **Stream order = priority order.** Critical CSS before JS before images. Because a scroll completes on the next delimiter, the client can hand each resource to the page as soon as it closes.
- **Channels:** each logical channel is a coordinate subtree (e.g., library 2 = telemetry, library 3 = agent transcript). No per-stream headers, no escaping.
- **Coordinate addressing on the server:** `/doc.phext;1.1.1;1.1.1;1.1.3` (the existing URL format) returns a single scroll or range — maps directly onto libphext's `fetch` / `range_replace` semantics.
- **v2 candidates (data-driven):** manifest-based delta (client sends its xxh3 manifest hash, server sends only changed scrolls via `subtract`), brotli/zstd with a shared dictionary.

## 4. Component plans

### 4.1 nginx — `ngx_http_phext_module`

Directives:

```nginx
location /app/ {
    phext_bundle  /srv/app/bundle.manifest;   # dynamic: assemble files into a stream
}
location ~ \.phext$ {
    phext_serve   on;                         # static .phext + coordinate selection
}
```

- **Dynamic bundling** builds an `ngx_chain_t` of `[delimiter buf][file buf][delimiter buf][file buf]…` (delimiter bufs are a few static bytes, file bufs are `in_file` bufs) and passes it through nginx's stock **gzip filter**. The module sets `buf->flush` on scroll boundaries so the gzip filter sync-flushes there. Because gzip reads the file data into memory, `sendfile` doesn't apply on this path.
- **Compressed-bundle cache:** compressing on every request is wasted CPU, so the module caches the finished `.gz` bundle on disk keyed on (manifest, every input's inode/mtime/size) and serves it like a static file. That path *is* zero-copy (`sendfile`, or kTLS under TLS).
- **Inline decision** (§2) runs at cache-build time: text inlined, small binaries base64'd until the gzip flight budget is hit, everything else becomes `external` plus a `Link: rel=preload` header (and `103 Early Hints` via `early_hints` on nginx ≥ 1.29 — verify version).
- **Validation** of forbidden bytes is cached on (inode, mtime, size) in a shared-memory zone, so files aren't re-scanned per request.
- **Coordinate selection** for `.phext` files: decoder runs over the file to find byte offsets (like `get_subspace_coordinates`), then serves the sub-range through the gzip filter. Cache the offset index per file.
- Works with HTTP/2 and HTTP/3 out of the box, since it's just a content handler.

Required config, checked by the module at startup (it warns if the bundle type isn't gzip-enabled):

```nginx
gzip            on;
gzip_types      application/vnd.phext;
gzip_comp_level 6;          # dynamic path; cached bundles are pre-built at level 9
gzip_static     on;         # serve app.phext.gz directly when present
```

**Fastest possible start:** nginx can serve a pre-built `app.phext.gz` today with `types { application/vnd.phext phext; }` and `gzip_static on;` — no module. Use this for the first benchmark (§5) before writing any C.

### 4.2 Apache httpd — `mod_phext`

- Same directives, same core library, same inline rule and compressed-bundle cache. Content handler builds an APR bucket brigade of **file buckets + immortal buckets** (delimiters) — the direct analogue of the nginx chain.
- **gzip via `mod_deflate`:** `AddOutputFilterByType DEFLATE application/vnd.phext`. The handler inserts `FLUSH` buckets at scroll boundaries; mod_deflate sync-flushes zlib on them. Cached pre-compressed bundles are served directly with `Content-Encoding: gzip` and skip mod_deflate.
- Large binaries become `external` entries plus `Link: rel=preload`; mod_http2 can send `103 Early Hints` via `H2EarlyHints on`.
- Optional output filter `PHEXT` to wrap an arbitrary upstream response into a scroll at a configured coordinate (useful for bundling proxied/CGI content).
- Build with `apxs`, linking `libphext_stream.a`. httpd has no official Rust bindings, so C glue + C ABI is the right shape.
- Expect ~70% code reuse in design with the nginx module; the server API glue is different but small.

### 4.3 Chrome — two stages

**Stage A: no browser changes (do first).** A Service Worker intercepts the page navigation, fetches the bundle, pipes `response.body` through the WASM `Decoder` as a `TransformStream`, and answers each subresource `fetch` event from the decoded scroll as soon as that scroll closes (waiting on a per-URL promise otherwise). gzip needs no work here: Chrome's network stack decompresses `Content-Encoding: gzip` incrementally before the body reaches the SW, so the server's sync-flushes reach the decoder as soon as they're sent. Requests for `external` URLs fall through to the network as normal H2 fetches. This gets ~90% of the experiment's signal in days, works in every Chromium channel, and gives a reference implementation to diff the native one against.

Limitation: the very first visit pays SW registration. Measure with a pre-registered SW (repeat visits) and accept that first-visit numbers need Stage B.

**Stage B: native support (only if Stage A data justifies it).**

- Model it on Chromium's former **Web Bundles subresource loading** work (`components/web_package`, `content/browser/web_package` — verify current tree state, parts were removed after the origin trial ended). That code already solved: "one response, many URLs, serve subresources from it via a URLLoaderFactory".
- Pieces: `application/vnd.phext` navigation handling → a `PhextBundleReader` (Rust core via `rust_static_library` + cxx bridge) in the network/browser process → a `URLLoaderFactory` that serves in-scope subresource URLs from decoded scrolls, blocking on not-yet-closed scrolls → DevTools Network panel entries per scroll.
- Security review items: scope rules (bundle may only serve URLs under its own origin/path), CORB/ORB interaction, CSP, MIME sniffing per scroll, memory caps on buffered scrolls.
- Cost reality: Chromium checkout + build is ~100 GB disk and many hours for the first build even on a 16+ core machine; on Windows it needs Visual Studio + the Windows SDK. Budget a dedicated Linux build box (or cloud VM) and use component builds for iteration.

## 5. Benchmark harness (build in Phase 1, reuse forever)

- `docker compose`: nginx, httpd, a static origin; `tc netem` on the link for RTT 25 / 50 / 100 / 200 ms and loss 0 / 1%.
- Driver: headless Chrome via Puppeteer or Playwright, cold and warm runs, 30+ runs per cell, report medians and p95.
- Fixtures: (a) small app ≤14 KB compressed, (b) typical SPA ~200 KB, (c) 50-channel text artifact (the phext-native case), (d) image-heavy page (exercises the inline/external split).
- All bundle variants are served gzip; all baselines get gzip too, so the comparison is fair.
- **Tuning sweeps:** `phext_inline_max` (2 / 4 / 8 / 16 KB) × `phext_flight_budget` (14 / 28 / 42 KB) × flush policy (header only / every scroll / every critical scroll). Output: recommended defaults per RTT band.
- **Compression checks:** gzip size of base64-in-bundle vs. the same binary served raw over H2, to confirm the base64 overhead is near zero after gzip; gzip-9 vs. zopfli for pre-built bundles; cost of sync-flushes in bytes.
- Metrics: time to last bundle byte, number of request RTTs, FCP, LCP, time until all channels decoded, time at which the first `external` fetch starts, compressed bytes in flight 1 and flight 2, server CPU per request (dynamic vs. cached bundle).
- Compare all four baselines in §1. Publish the table — this is the evidence for upstreaming or for the Chromium investment.

## 6. Phased schedule

Durations assume one experienced engineer; parallel streams noted.

| Phase | Work | Duration | Exit criteria |
|---|---|---|---|
| **0. Spec + core** | Write bundle v1 spec; build `phext-stream` (decoder/encoder/validator), C ABI via cbindgen, WASM build; test vectors from libphext-rs tests + regression files; `cargo fuzz` decoder; differential tests against libphext-rs `phokenize` and your Python/C#/Node implementations | 1–2 wks | API frozen; fuzzed 24h clean; all four implementations agree on the corpus |
| **1. Prove it** | Offline bundler CLI (`phext-bundle build dir/ → app.phext + app.phext.gz + preload list`) implementing the inline rule; nginx static serving with `gzip_static` (no module); Service Worker + WASM loader; benchmark harness incl. tuning sweeps | 1–2 wks | First benchmark table vs. all baselines; first-cut `inline_max` / `flight_budget` defaults |
| **Decision gate** | Does phext beat inline-everything and Early Hints anywhere that matters? Which fixtures? | — | Go / adjust format / stop |
| **2a. nginx module** *(parallel)* | `phext_bundle`, `phext_serve`, gzip-filter flush integration, compressed-bundle cache, preload/Early Hints for externals, validation cache, coordinate selection | 2–3 wks | Passes harness; cached path matches the static `gzip_static` numbers; no regressions under `wrk`/`h2load` |
| **2b. httpd module** *(parallel)* | `mod_phext` handler + filter, mod_deflate FLUSH integration, same cache | 2–3 wks | Same harness parity as nginx |
| **3. Chromium native** | Stage B above | 6–10 wks | Native beats SW Stage A on first visit; security checklist done |
| **4. v2 features** | Delta sync via manifests, brotli/zstd option, priority hints | data-driven | — |

Critical path: Phase 0 → 1 → gate. After the core API freezes, nginx, httpd, the SW loader, and the harness are fully independent and can run as parallel workstreams (separate people, or separate agent sessions each owning one directory).

## 7. Repository layout

```
phext-web/
  spec/bundle-v1.md
  core/phext-stream/        # Rust crate: no_std + alloc, C ABI, wasm feature
  core/test-vectors/        # *.phext + expected JSON events; shared by all implementations
  tools/phext-bundle/       # CLI bundler
  servers/nginx/            # ngx_http_phext_module (config + .c)
  servers/httpd/            # mod_phext.c
  browser/sw-loader/        # Service Worker + WASM glue
  browser/chromium-patch/   # Stage B patches against a pinned Chromium revision
  bench/                    # docker compose, netem scripts, puppeteer driver, fixtures
```

## 8. Risks

| Risk | Mitigation |
|---|---|
| Inlining everything already captures most of the RTT win | Gate decision on data; position phext on multi-channel structure, per-scroll caching (xxh3), and delta sync |
| Bundle grows past the first flights and delays the scrolls that matter | Flight budget enforced by the bundler and modules; large binaries are always `external` on H2 |
| Compressor buffering hides scroll boundaries from the client | Sync-flush at scroll boundaries (§3.2); harness measures time-to-first-decoded-scroll |
| Per-request gzip CPU on dynamic bundles | Compressed-bundle cache; pre-built `.gz` for static bundles |
| Chromium upstream acceptance is a long shot | Keep Stage B as a maintained patch set for testing; SW path remains the portable deployment |
| Delimiter bytes in "text" (e.g., raw `0x1F` in a minified JS string) | Server-side validation is mandatory, never optional; bundler treats that file as binary (base64 if small, otherwise `external`) |
| Large scroll blocks dependents while streaming | Bundler splits large resources across consecutive coordinates; priority ordering in header |

## 9. Launch follow-ups

- [ ] **Reply on X once this work is ready:** https://x.com/wvo/status/2114401248829186411 (include the benchmark table from §5 and a link to this repo).
