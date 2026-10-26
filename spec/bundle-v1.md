# Phext Bundle v1 — Wire Format

Status: draft for the Phase 0/1 experiment. Normative words (MUST, SHOULD, MAY) follow RFC 2119.

## 1. Phext stream

A phext stream is a sequence of bytes. Every byte is either content or one of nine **dimension breaks**:

| Byte | Break | Effect on the current coordinate |
|---|---|---|
| `0x17` | scroll | scroll += 1 |
| `0x18` | section | section += 1; scroll = 1 |
| `0x19` | chapter | chapter += 1; section, scroll = 1 |
| `0x1A` | book | book += 1; all lower = 1 |
| `0x1C` | volume | volume += 1; all lower = 1 |
| `0x1D` | collection | collection += 1; all lower = 1 |
| `0x1E` | series | series += 1; all lower = 1 |
| `0x1F` | shelf | shelf += 1; all lower = 1 |
| `0x01` | library | library += 1; all lower = 1 |

The coordinate starts at `1.1.1/1.1.1/1.1.1` (`library.shelf.series/collection.volume.book/chapter.section.scroll`). Line feed `0x0A` and every other byte are content.

**Saturation (libphext 0.3.x compatibility):** a dimension never advances past **999**; a break that would move it to 1000 leaves that dimension unchanged (lower dimensions are still reset). Encoders MUST NOT produce coordinates with any dimension above 999.

A **scroll** is a maximal non-empty run of content bytes; its coordinate is the coordinate in effect when its first byte arrives. Runs of consecutive breaks produce no scrolls. A scroll is **complete** when the next break byte arrives or the stream ends.

Decoders MUST treat content as opaque bytes (no UTF-8 requirement) and MUST NOT buffer: everything needed to place a byte is the current coordinate.

## 2. Encoding

Scrolls MUST be emitted in strictly increasing coordinate order (lexicographic over library … scroll). Between scroll A and scroll B the encoder emits the **minimal break sequence**: repeatedly, take the highest dimension where the current coordinate is below B, emit that dimension's break, and apply it — until the current coordinate equals B.

Scroll content MUST NOT contain any of the nine break bytes. Encoders MUST validate content and reject (or reroute, §4) any that does.

## 3. Bundle

- Media type: `application/vnd.phext` (experimental alias: `application/x-phext`).
- Servers MUST send bundles with `Content-Encoding: gzip`. Bundles are compressed **one-shot** by default: a single gzip stream with no flush points, which is the smallest output. The bundle is sized (§3.2) to arrive within the round trips HTTP/2 would spend anyway, so it doesn't need intermediate flushes. Encoders MAY sync-flush at scroll boundaries as an option. Pre-compressed bundles are the preferred serving path.

### 3.1 Header scroll

Scroll `1.1.1/1.1.1/1.1.1` MUST be the header. It is UTF-8 text, lines separated by `\n` (a trailing `\r` on a line is ignored):

```
phext-bundle 1
<coordinate|external>  <media-type>[;base64]  <path>
...
```

- Line 1 is exactly `phext-bundle 1`.
- Each following non-empty line has three fields separated by ASCII whitespace. `path` is the URL path the resource answers to (absolute, starting with `/`; may not contain whitespace).
- `coordinate` is `a.b.c/d.e.f/g.h.i` or the URL form `a.b.c;d.e.f;g.h.i`. Each coordinate MUST be greater than `1.1.1/1.1.1/1.1.1` and MUST appear at most once.
- `;base64` after the media type means the scroll content is standard base64 (RFC 4648 §4, padding required, no line breaks) and the client MUST decode it before use.
- `external` means the resource is not in the bundle; the client fetches `path` normally. Servers SHOULD also advertise external resources with `Link: <path>; rel=preload` (and `103 Early Hints` where supported).
- Lines are in priority order. Content scrolls SHOULD appear in the stream in the same order as their header lines; since coordinates must increase, bundlers assign coordinates in priority order.
- Unknown lines starting with `#` are comments. Decoders MUST ignore them.
- Decoders MAY cap the header's size, but MUST accept at least 1 MiB (a 2,500-entry header is ~100 KiB).

A resource listed with a coordinate but absent from the stream (no scroll at that coordinate by end of stream) is an error for that resource only; clients SHOULD fall back to fetching `path`.

### 3.2 Inline rule (bundler and server modules)

**Default: inline everything.** Every non-empty asset goes in the bundle. Text that passes validation travels as-is; binaries, and text containing break bytes, travel as `;base64` scrolls. Empty assets are listed as `external`, because an empty scroll is invisible on the wire.

Rationale (measured in `bench/RESULTS.md`): an external resource can't be requested until the header scroll arrives, so it costs at least one more round trip, plus slow start on that request. Bytes added to the bundle ride a congestion window that is already open. Inlining everything gave the fastest (or tied) complete page in every fixture that had binaries, over HTTP/1.1 and HTTP/2, at 20–200 ms RTT.

**Budgeted mode (optional).** Encoders MAY leave binaries external to keep the bundle small, for example for an earlier first paint or so large assets can be cached separately:

```
inline(asset) =  is_text(asset) && validate(asset) == ok
              || ( base64_len(asset) <= inline_max
                   && gzip_len(bundle + asset) <= flight_budget )
```

A natural budget is what TCP slow start delivers in the round trips HTTP/2 spends after its handshake: one for the document, one for the multiplexed subresources. That is `initcwnd × (2² − 1)` = 3 × 10 segments × 1460 = **43,800 bytes** with the standard 10-segment initial window. Where bandwidth rather than slow start is the limit, use `min(that, bandwidth × 2 RTT)`; other round counts give `initcwnd × (2ⁿ − 1)`.

Assets are considered in priority order. A text asset that fails validation is treated as binary. Everything not inlined is `external`.

## 4. Coordinate addressing (server)

`GET /doc.phext;a.b.c;d.e.f;g.h.i` returns the scroll at that coordinate as `text/plain` (or the media type known from a bundle header). Servers MAY support ranges `start-end` in the same notation later.
