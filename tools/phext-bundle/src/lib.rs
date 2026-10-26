//! Builds a phext bundle (spec/bundle-v1.md) from a set of assets, applying the inline rule from §3.2.
//! By default everything travels inline: text unescaped, binaries (and text containing break bytes) as
//! base64. Benchmarks showed one bundle beats leaving anything to a second round of requests
//! (bench/RESULTS.md). Optional limits (`inline_max`, `flight_budget`) restore budgeted mode, where a
//! binary is inlined only if it is small and the gzip'd bundle still fits; the rest is listed as `external`
//! for the client to fetch over plain HTTP/2.

pub mod codecs;

use flate2::write::GzEncoder;
use flate2::Compression;
use phext_stream::{base64, validate, Coordinate, Encoder, Entry, Header, Location};
use std::io::{self, Write};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Asset {
    /// URL path, starting with `/`.
    pub path: String,
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
}

impl Asset {
    pub fn new(path: impl Into<String>, bytes: Vec<u8>) -> Asset {
        let path = path.into();
        Asset {
            media_type: media_type(&path),
            path,
            bytes,
        }
    }

    pub fn is_text(&self) -> bool {
        is_text_type(self.media_type)
    }
}

/// One TCP segment's payload, the unit of the initial congestion window.
pub const SEGMENT: usize = 1460;
/// Linux's default initial congestion window (RFC 6928): 10 segments.
pub const INITCWND: usize = 10 * SEGMENT;
/// Round trips in HTTP/2's request -> response -> request cycle after the handshake (the document, then
/// one multiplexed round of subresources): the budget to use with `--budget-rounds` when you want
/// budgeted mode sized to what HTTP/2 would spend anyway.
pub const BUDGET_ROUNDS: u32 = 2;

/// No limit: the default for both `inline_max` and `flight_budget` (inline everything).
pub const UNLIMITED: usize = usize::MAX;

/// Bytes TCP slow start delivers in `rounds` round trips from a fresh connection, when bandwidth is not the
/// limit: initcwnd + 2*initcwnd + ... = initcwnd * (2^rounds - 1). Two rounds is 43,800 bytes, which is what
/// the bundle can carry before HTTP/2 would even have its first subresource responses back.
pub const fn slow_start_bytes(rounds: u32, initcwnd: usize) -> usize {
    initcwnd * ((1usize << rounds) - 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flush {
    /// One-shot: a single gzip stream with no flush points. The default: smallest output, and per-scroll
    /// flushing made no measurable difference to load time (bench/RESULTS.md).
    None,
    /// Sync-flush once, right after the header scroll closes, so externals can start immediately.
    Header,
    /// Sync-flush after every scroll closes, so each resource completes as soon as its bytes are sent.
    All,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub inline_max: usize,
    pub flight_budget: usize,
    pub flush: Flush,
    pub level: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            inline_max: UNLIMITED,
            flight_budget: UNLIMITED,
            flush: Flush::None,
            level: 9,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Text,
    Base64,
    /// Not inlined; the reason is for the report.
    External(&'static str),
}

#[derive(Debug)]
pub struct Built {
    pub header: Header,
    pub decisions: Vec<Decision>,
    pub phext: Vec<u8>,
    pub gz: Vec<u8>,
    /// `Link` header values for the external entries, in priority order.
    pub links: Vec<String>,
}

/// The coordinate of the n-th inline resource (0-based), right after the header: scrolls 2..=999 of
/// section 1, then on through further sections.
fn nth_coordinate(n: usize) -> Coordinate {
    let k = n + 1; // the header is k = 0
    let per = phext_stream::MAX_INDEX as usize;
    let mut d = [1u16; 9];
    d[8] = (k % per + 1) as u16;
    d[7] = (k / per + 1) as u16;
    Coordinate(d)
}

fn assemble(assets: &[Asset], decisions: &[Decision], opts: &Options) -> io::Result<Built> {
    let mut entries = Vec::with_capacity(assets.len());
    let mut scrolls: Vec<(Coordinate, Vec<u8>)> = Vec::new();
    for (a, d) in assets.iter().zip(decisions) {
        let location = match d {
            Decision::External(_) => Location::External,
            _ => {
                let c = nth_coordinate(scrolls.len());
                let body = if *d == Decision::Base64 {
                    base64::encode(&a.bytes).into_bytes()
                } else {
                    a.bytes.clone()
                };
                scrolls.push((c, body));
                Location::Inline(c)
            }
        };
        entries.push(Entry {
            location,
            media_type: a.media_type.to_string(),
            base64: *d == Decision::Base64,
            path: a.path.clone(),
        });
    }
    let header = Header { entries };

    // Plain stream first, remembering where each scroll closes on the wire: at the first break byte of
    // the run leading to the next scroll. Flushing gzip right after that byte lets the client finish the
    // resource without waiting for the compressor.
    let mut phext = Vec::new();
    let mut closes = Vec::new();
    let mut enc = Encoder::new();
    let header_text = header.encode();
    let all = std::iter::once((Coordinate::ORIGIN, header_text.as_bytes()))
        .chain(scrolls.iter().map(|(c, b)| (*c, b.as_slice())));
    for (k, (at, body)) in all.enumerate() {
        let breaks = enc.seek(at).map_err(io::Error::other)?;
        if k > 0 {
            closes.push(phext.len() + 1);
        }
        phext.extend(breaks);
        phext.extend_from_slice(body);
    }
    let flush_at: &[usize] = match opts.flush {
        Flush::None => &[],
        Flush::Header => &closes[..closes.len().min(1)],
        Flush::All => &closes,
    };
    let mut gz = GzEncoder::new(Vec::new(), Compression::new(opts.level));
    let mut prev = 0;
    for &at in flush_at {
        gz.write_all(&phext[prev..at])?;
        gz.flush()?;
        prev = at;
    }
    gz.write_all(&phext[prev..])?;
    let gz = gz.finish()?;

    let links = assets
        .iter()
        .zip(decisions)
        .filter(|(_, d)| matches!(d, Decision::External(_)))
        .map(|(a, _)| preload_link(a))
        .collect();
    Ok(Built {
        header,
        decisions: decisions.to_vec(),
        phext,
        gz,
        links,
    })
}

/// Applies the inline rule (spec §3.2) to `assets`, which must be in priority order.
pub fn build(assets: &[Asset], opts: &Options) -> io::Result<Built> {
    let mut decisions: Vec<Decision> = assets
        .iter()
        .map(|a| {
            if a.bytes.is_empty() {
                // an empty scroll is invisible on the wire, so the client would treat it as missing
                Decision::External("empty")
            } else if a.is_text() && validate(&a.bytes).is_ok() {
                Decision::Text
            } else if base64::encoded_len(a.bytes.len()) > opts.inline_max {
                Decision::External("over inline_max")
            } else {
                Decision::External("pending")
            }
        })
        .collect();
    // greedy in priority order: keep each small binary only if the whole gzip'd bundle still fits
    for i in 0..assets.len() {
        if decisions[i] != Decision::External("pending") {
            continue;
        }
        decisions[i] = Decision::Base64;
        if opts.flight_budget == UNLIMITED {
            continue; // nothing to check, and no need to compress a trial bundle per asset
        }
        let trial = assemble(assets, &decisions, opts)?;
        if trial.gz.len() > opts.flight_budget {
            decisions[i] = Decision::External("over flight_budget");
        }
    }
    assemble(assets, &decisions, opts)
}

fn preload_link(a: &Asset) -> String {
    let t = a.media_type;
    let r#as = if t.starts_with("image/") {
        "image"
    } else if t.starts_with("font/") {
        "font"
    } else if t == "text/css" {
        "style"
    } else if t == "text/javascript" {
        "script"
    } else {
        "fetch"
    };
    let cors = if r#as == "font" || r#as == "fetch" {
        "; crossorigin"
    } else {
        ""
    };
    format!("<{}>; rel=preload; as={}{}", a.path, r#as, cors)
}

pub fn media_type(path: &str) -> &'static str {
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "xml" => "application/xml",
        "phext" => "text/phext",
        "wasm" => "application/wasm",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}

pub fn is_text_type(t: &str) -> bool {
    t.starts_with("text/") || matches!(t, "application/json" | "application/xml" | "image/svg+xml")
}

/// Default priority when no order file is given: the entry document, then CSS, JS, other text, then
/// binaries smallest first (they are the ones that can still fit the budget).
fn default_rank(a: &Asset, entry: &str) -> (u8, usize) {
    let t = a.media_type;
    let class = if a.path == entry {
        0
    } else if t == "text/css" {
        1
    } else if t == "text/javascript" {
        2
    } else if a.is_text() {
        3
    } else {
        4
    };
    (class, if class == 4 { a.bytes.len() } else { 0 })
}

/// Reads every non-hidden file under `dir`. `order` lists URL paths (or paths relative to `dir`) that come
/// first, in that order; the rest follow in the default priority.
pub fn collect(dir: &Path, entry: &str, order: &[String]) -> io::Result<Vec<Asset>> {
    let mut assets = Vec::new();
    walk(dir, dir, &mut assets)?;
    let entry = format!("/{}", entry.trim_start_matches('/'));
    let pos = |a: &Asset| {
        order
            .iter()
            .position(|o| format!("/{}", o.trim_start_matches('/')) == a.path)
    };
    assets.sort_by(|a, b| match (pos(a), pos(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => default_rank(a, &entry)
            .cmp(&default_rank(b, &entry))
            .then_with(|| a.path.cmp(&b.path)),
    });
    Ok(assets)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<Asset>) -> io::Result<()> {
    let mut items: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    items.sort_by_key(|e| e.file_name());
    for item in items {
        let name = item.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let p = item.path();
        if item.file_type()?.is_dir() {
            walk(root, &p, out)?;
        } else {
            let rel = p.strip_prefix(root).map_err(io::Error::other)?;
            let url = format!("/{}", rel.to_string_lossy().replace('\\', "/"));
            if url
                .chars()
                .any(|c| c.is_ascii_whitespace() || c.is_ascii_control())
            {
                return Err(io::Error::other(format!(
                    "{url}: paths with whitespace can't go in a bundle header"
                )));
            }
            out.push(Asset::new(url, std::fs::read(&p)?));
        }
    }
    Ok(())
}
