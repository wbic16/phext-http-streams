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

Phase 0 (spec + core) done. Next: Phase 1 — bundler CLI with the inline rule and gzip, static nginx serving,
Service Worker + WASM loader, and the benchmark harness.
