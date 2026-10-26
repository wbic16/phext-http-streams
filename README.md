# phext-http-streams

Native phext streaming for nginx, Apache httpd, and Chrome: deliver a whole multi-resource artifact as one
gzip'd phext stream in the first response flight, instead of a discover-then-fetch waterfall.

- Plan: [phext-streaming-plan.md](phext-streaming-plan.md)
- Wire format: [spec/bundle-v1.md](spec/bundle-v1.md)

## Layout

| Path | What |
|---|---|
| `core/phext-stream` | Rust core: zero-copy streaming `Decoder`, `Encoder`, `validate`, `find`/`index`, bundle `Header`, streaming `BundleReader`, base64. `no_std`; `alloc` feature (default) for the bundle layer. |
| `core/phext-stream-capi` | C ABI (`include/phext_stream.h`) as a static library for the nginx/httpd modules. |
| `core/test-vectors` | Language-neutral vectors plus checkers for the Python, Node, and C# implementations. |
| `fuzz` | cargo-fuzz target (Linux/macOS, nightly). |
| `tools/phext-bundle` | CLI: directory in, gzip'd bundle + report out. Inlines everything by default (binaries as base64); budgeted mode leaves some binaries external with preload links. |
| `core/phext-stream-wasm` | The bundle reader as a dependency-free WebAssembly module. |
| `browser/sw-loader` | Service Worker that serves a page from a phext bundle (`node browser/sw-loader/build.mjs`). |
| `servers/nginx` | Static serving config for stock nginx, plus a check script. |
| `bench` | HTTP/1.1 vs HTTP/2 delivery benchmark: network model, origin, fixtures, Chrome runner. [Results](bench/RESULTS.md). |

## Build and test

```bash
cargo test
```

```bash
pwsh core/phext-stream-capi/tests/c/run.ps1
```

```bash
cargo run --release -p phext-stream --example throughput
```

`cargo test` includes randomized differential tests against libphext 0.3.1 (decode, encode, find,
coordinate formatting) and a fuzz-style property test over arbitrary bytes. The C test builds the static
library and drives it from C via MSVC; on Linux see the comment at the top of `test_capi.c`.

## Status

Phase 0 (spec + core) and Phase 1 (bundler, static nginx, Service Worker loader, benchmark) are done; see
[bench/RESULTS.md](bench/RESULTS.md). With everything in one phext response, HTTP/1.1 runs within 4% of
HTTP/2 and 27–56% faster than HTTP/2 serving plain files, across 4 fixtures at 20–200 ms RTT. Gzip flushing
and the choice of encoder made no measurable difference to load time; a bigger inline budget did. Next:
first-visit measurements, then Phase 2 server modules. Bundles inline everything by default.
