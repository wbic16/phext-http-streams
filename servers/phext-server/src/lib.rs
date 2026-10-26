//! C ABI for the nginx and httpd modules: turn a directory into a phext bundle, pre-encoded once per
//! content-encoding, and tell cheaply whether the directory has changed since. The header is
//! `include/phext_server.h`; keep the two in sync.
//!
//! A module links exactly one Rust static library, so this crate also re-exports the phext-stream C ABI
//! (`phext_stream.h`: decoder, encoder, `phext_find`, ...).
//!
//! Conventions as in phext_stream.h: pointer arguments may be NULL (the call fails cleanly), nothing
//! panics across the boundary, and a built `phext_site` is immutable, so threads may share it freely.

#![allow(clippy::missing_safety_doc, non_camel_case_types)]

pub use phext_stream_c::*;

use core::ffi::{c_char, c_int};
use phext_bundle::codecs::Codec;
use phext_bundle::{build, collect, Asset, Options, UNLIMITED};
use std::ffi::CStr;
use std::hash::{Hash, Hasher};
use std::path::Path;

pub const PHEXT_ENC_IDENTITY: c_int = 0;
pub const PHEXT_ENC_GZIP: c_int = 1;
pub const PHEXT_ENC_BR: c_int = 2;
pub const PHEXT_ENC_ZSTD: c_int = 3;
const ENC_TOKENS: [&str; 4] = ["identity", "gzip", "br", "zstd"];

#[repr(C)]
pub struct phext_build_opts {
    /// Entry document, relative to the directory (NULL = "index.html").
    pub entry: *const c_char,
    /// Optional priority order file: one URL path per line (NULL = default ordering).
    pub order_file: *const c_char,
    /// gzip via libdeflate, 1..=12; 0 = don't produce gzip.
    pub gzip_level: c_int,
    /// brotli quality 0..=11; -1 = don't produce br.
    pub brotli_quality: c_int,
    /// zstd level 1..=22; 0 = don't produce zstd.
    pub zstd_level: c_int,
    /// Budgeted mode (spec §3.2); SIZE_MAX for both = inline everything (the default).
    pub inline_max: usize,
    pub flight_budget: usize,
}

/// A built bundle: the plain stream plus each requested encoding.
pub struct phext_site {
    fingerprint: u64,
    bodies: [Option<Vec<u8>>; 4],
    links: Vec<u8>,
    entries: usize,
}

/// Fills `opts` with the defaults: inline everything; gzip 6, br 5, zstd 3 (fast enough to build on a
/// request path; pre-build with `phext-bundle` for maximum compression).
#[no_mangle]
pub unsafe extern "C" fn phext_build_opts_default(opts: *mut phext_build_opts) {
    if let Some(o) = opts.as_mut() {
        *o = phext_build_opts {
            entry: std::ptr::null(),
            order_file: std::ptr::null(),
            gzip_level: 6,
            brotli_quality: 5,
            zstd_level: 3,
            inline_max: UNLIMITED,
            flight_budget: UNLIMITED,
        };
    }
}

unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        None
    } else {
        CStr::from_ptr(p).to_str().ok()
    }
}

/// Precompressed siblings and hidden files never go into a bundle.
fn skipped(path: &str) -> bool {
    path.rsplit('/').next().is_some_and(|n| n.starts_with('.'))
        || [".gz", ".br", ".zst"].iter().any(|s| path.ends_with(s))
}

fn hash_tree(root: &Path, dir: &Path, h: &mut impl Hasher) -> std::io::Result<()> {
    let mut items: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    items.sort_by_key(|e| e.file_name());
    for item in items {
        let p = item.path();
        let rel = p
            .strip_prefix(root)
            .map_err(std::io::Error::other)?
            .to_string_lossy()
            .replace('\\', "/");
        if skipped(&rel) {
            continue;
        }
        let md = item.metadata()?;
        if md.is_dir() {
            hash_tree(root, &p, h)?;
        } else {
            rel.hash(h);
            md.len().hash(h);
            if let Ok(t) = md.modified() {
                t.hash(h);
            }
        }
    }
    Ok(())
}

fn fingerprint(dir: &Path, order_file: Option<&str>) -> std::io::Result<u64> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    hash_tree(dir, dir, &mut h)?;
    if let Some(f) = order_file {
        let md = std::fs::metadata(f)?;
        (f, md.len(), md.modified().ok()).hash(&mut h);
    }
    // never 0, so 0 can mean "error"
    Ok(h.finish().max(1))
}

/// Cheap change detection: a hash of every bundled file's path, size, and mtime (plus the order file's).
/// One directory walk, no reads. Returns 0 on error.
#[no_mangle]
pub unsafe extern "C" fn phext_dir_fingerprint(
    dir: *const c_char,
    order_file: *const c_char,
) -> u64 {
    let Some(dir) = cstr(dir) else { return 0 };
    fingerprint(Path::new(dir), cstr(order_file)).unwrap_or(0)
}

fn build_site(dir: &str, o: &phext_build_opts) -> Result<phext_site, String> {
    let dir_path = Path::new(dir);
    let order_file = unsafe { cstr(o.order_file) };
    let fp = fingerprint(dir_path, order_file).map_err(|e| format!("{dir}: {e}"))?;
    let order: Vec<String> = match order_file {
        Some(f) => std::fs::read_to_string(f)
            .map_err(|e| format!("{f}: {e}"))?
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(String::from)
            .collect(),
        None => Vec::new(),
    };
    let entry = unsafe { cstr(o.entry) }.unwrap_or("index.html");
    let assets: Vec<Asset> = collect(dir_path, entry, &order)
        .map_err(|e| format!("{dir}: {e}"))?
        .into_iter()
        .filter(|a| !skipped(&a.path))
        .collect();
    let opts = Options {
        inline_max: o.inline_max,
        flight_budget: o.flight_budget,
        flate2: false,
        ..Options::default()
    };
    let built = build(&assets, &opts).map_err(|e| e.to_string())?;
    let encode = |c: Codec| c.compress(&built.phext).map_err(|e| e.to_string());
    let gz = (o.gzip_level > 0)
        .then(|| encode(Codec::Libdeflate(o.gzip_level.clamp(1, 12))))
        .transpose()?;
    let br = (o.brotli_quality >= 0)
        .then(|| encode(Codec::Brotli(o.brotli_quality.clamp(0, 11) as u32)))
        .transpose()?;
    let zst = (o.zstd_level > 0)
        .then(|| encode(Codec::Zstd(o.zstd_level.clamp(1, 22))))
        .transpose()?;
    Ok(phext_site {
        fingerprint: fp,
        entries: built.header.entries.len(),
        links: built.links.join(", ").into_bytes(),
        bodies: [Some(built.phext), gz, br, zst],
    })
}

fn write_err(err: *mut c_char, cap: usize, msg: &str) {
    if err.is_null() || cap == 0 {
        return;
    }
    let n = msg.len().min(cap - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(msg.as_ptr(), err as *mut u8, n);
        *err.add(n) = 0;
    }
}

/// Builds the bundle for `dir` and every encoding `opts` asks for. Returns 0 and a site in `*out` (free it
/// with `phext_site_free`), or -1 with a message in `err` (NUL-terminated, truncated to `err_cap`).
/// `opts` may be NULL for the defaults.
#[no_mangle]
pub unsafe extern "C" fn phext_site_build(
    dir: *const c_char,
    opts: *const phext_build_opts,
    out: *mut *mut phext_site,
    err: *mut c_char,
    err_cap: usize,
) -> c_int {
    let Some(out) = out.as_mut() else { return -1 };
    let Some(dir) = cstr(dir) else {
        write_err(err, err_cap, "phext: directory is NULL or not UTF-8");
        return -1;
    };
    let mut defaults = std::mem::zeroed::<phext_build_opts>();
    phext_build_opts_default(&mut defaults);
    let o = opts.as_ref().unwrap_or(&defaults);
    match std::panic::catch_unwind(|| build_site(dir, o)) {
        Ok(Ok(site)) => {
            *out = Box::into_raw(Box::new(site));
            0
        }
        Ok(Err(msg)) => {
            write_err(err, err_cap, &format!("phext: {msg}"));
            -1
        }
        Err(_) => {
            write_err(
                err,
                err_cap,
                "phext: internal error while building the bundle",
            );
            -1
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn phext_site_free(site: *mut phext_site) {
    if !site.is_null() {
        drop(Box::from_raw(site));
    }
}

#[no_mangle]
pub unsafe extern "C" fn phext_site_fingerprint(site: *const phext_site) -> u64 {
    site.as_ref().map_or(0, |s| s.fingerprint)
}

/// Number of entries in the bundle header.
#[no_mangle]
pub unsafe extern "C" fn phext_site_entries(site: *const phext_site) -> usize {
    site.as_ref().map_or(0, |s| s.entries)
}

/// The body for `encoding` (PHEXT_ENC_*), or NULL if that encoding wasn't built. Valid until the site is
/// freed.
#[no_mangle]
pub unsafe extern "C" fn phext_site_body(
    site: *const phext_site,
    encoding: c_int,
    len: *mut usize,
) -> *const u8 {
    let body = site
        .as_ref()
        .and_then(|s| s.bodies.get(usize::try_from(encoding).ok()?)?.as_ref());
    if let Some(l) = len.as_mut() {
        *l = body.map_or(0, Vec::len);
    }
    body.map_or(std::ptr::null(), |b| b.as_ptr())
}

/// `Link: preload` value for external entries (comma-joined; empty when everything is inline).
#[no_mangle]
pub unsafe extern "C" fn phext_site_links(
    site: *const phext_site,
    len: *mut usize,
) -> *const c_char {
    let links = site.as_ref().map(|s| &s.links);
    if let Some(l) = len.as_mut() {
        *l = links.map_or(0, Vec::len);
    }
    links.map_or(std::ptr::null(), |l| l.as_ptr() as *const c_char)
}

/// Content-Encoding token for PHEXT_ENC_* ("identity", "gzip", "br", "zstd"); NUL-terminated, static.
#[no_mangle]
pub extern "C" fn phext_encoding_token(encoding: c_int) -> *const c_char {
    match encoding {
        PHEXT_ENC_GZIP => c"gzip".as_ptr(),
        PHEXT_ENC_BR => c"br".as_ptr(),
        PHEXT_ENC_ZSTD => c"zstd".as_ptr(),
        _ => c"identity".as_ptr(),
    }
}

/// Writes the strong ETag for this site and encoding, quotes included, like snprintf: returns the length
/// excluding NUL and writes only if it fits. At most 26 bytes.
#[no_mangle]
pub unsafe extern "C" fn phext_site_etag(
    site: *const phext_site,
    encoding: c_int,
    buf: *mut c_char,
    cap: usize,
) -> usize {
    let Some(s) = site.as_ref() else { return 0 };
    let token = ENC_TOKENS
        .get(encoding as usize)
        .copied()
        .unwrap_or("identity");
    let tag = format!("\"{:016x}-{}\"", s.fingerprint, &token[..2]);
    if !buf.is_null() && tag.len() < cap {
        std::ptr::copy_nonoverlapping(tag.as_ptr(), buf as *mut u8, tag.len());
        *buf.add(tag.len()) = 0;
    }
    tag.len()
}

/// q-value the client gives `token` in an Accept-Encoding header (`*` counts; absent = 0, except identity,
/// which is acceptable unless excluded).
fn q_for(accept: &str, token: &str) -> f32 {
    let mut star = None;
    for part in accept.split(',') {
        let mut it = part.split(';');
        let name = it.next().unwrap_or("").trim();
        let q = it
            .find_map(|p| {
                p.trim()
                    .strip_prefix("q=")
                    .and_then(|v| v.trim().parse::<f32>().ok())
            })
            .unwrap_or(1.0);
        if name.eq_ignore_ascii_case(token) {
            return q;
        }
        if name == "*" {
            star = Some(q);
        }
    }
    star.unwrap_or(if token == "identity" { 1.0 } else { 0.0 })
}

/// Chooses the encoding to send: of the bodies this site has and the client accepts (q > 0), the smallest.
/// `accept` is the raw Accept-Encoding value (`len` bytes; NULL = header absent, which means identity).
/// Returns -1 if the client refuses everything available (send 406).
#[no_mangle]
pub unsafe extern "C" fn phext_site_pick(
    site: *const phext_site,
    accept: *const c_char,
    len: usize,
) -> c_int {
    let Some(s) = site.as_ref() else { return -1 };
    let accept = if accept.is_null() {
        ""
    } else {
        std::str::from_utf8(std::slice::from_raw_parts(accept as *const u8, len)).unwrap_or("")
    };
    let mut best: Option<(usize, c_int)> = None;
    for (enc, body) in s.bodies.iter().enumerate() {
        let Some(body) = body else { continue };
        if q_for(accept, ENC_TOKENS[enc]) <= 0.0 {
            continue;
        }
        if best.is_none_or(|(n, _)| body.len() < n) {
            best = Some((body.len(), enc as c_int));
        }
    }
    best.map_or(-1, |(_, e)| e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn site_for(dir: &Path) -> *mut phext_site {
        let d = CString::new(dir.to_str().unwrap()).unwrap();
        let mut site = std::ptr::null_mut();
        let mut err = [0 as c_char; 256];
        let rc = unsafe {
            phext_site_build(
                d.as_ptr(),
                std::ptr::null(),
                &mut site,
                err.as_mut_ptr(),
                err.len(),
            )
        };
        assert_eq!(rc, 0, "{:?}", unsafe { CStr::from_ptr(err.as_ptr()) });
        site
    }

    #[test]
    fn builds_encodes_and_picks() {
        let dir = std::env::temp_dir().join(format!("phext-server-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("css")).unwrap();
        std::fs::write(
            dir.join("index.html"),
            "<!doctype html><link rel=stylesheet href=/css/a.css>".repeat(40),
        )
        .unwrap();
        std::fs::write(dir.join("css/a.css"), "body{color:red}\n".repeat(200)).unwrap();
        std::fs::write(
            dir.join("logo.bmp"),
            (0..3000u32)
                .map(|i| (i.wrapping_mul(2654435761u32) >> 24) as u8)
                .collect::<Vec<u8>>(),
        )
        .unwrap();
        std::fs::write(dir.join("css/a.css.gz"), b"ignored").unwrap();
        std::fs::write(dir.join(".hidden"), b"ignored").unwrap();

        let site = site_for(&dir);
        unsafe {
            assert_eq!(
                phext_site_entries(site),
                3,
                "precompressed and hidden files are skipped"
            );
            let mut n = 0;
            let plain =
                std::slice::from_raw_parts(phext_site_body(site, PHEXT_ENC_IDENTITY, &mut n), n)
                    .to_vec();
            for enc in [PHEXT_ENC_GZIP, PHEXT_ENC_BR, PHEXT_ENC_ZSTD] {
                let p = phext_site_body(site, enc, &mut n);
                assert!(!p.is_null() && n < plain.len());
                let codec = [
                    Codec::Flate2(0),
                    Codec::Flate2(0),
                    Codec::Brotli(0),
                    Codec::Zstd(1),
                ][enc as usize];
                assert_eq!(
                    codec
                        .decompress(std::slice::from_raw_parts(p, n), plain.len())
                        .unwrap(),
                    plain
                );
            }
            let mut links = 0;
            phext_site_links(site, &mut links);
            assert_eq!(links, 0, "inline everything: no externals");

            let pick = |h: &str| phext_site_pick(site, h.as_ptr() as *const c_char, h.len());
            let len = |e: c_int| {
                let mut n = 0;
                phext_site_body(site, e, &mut n);
                n
            };
            let smallest = [PHEXT_ENC_GZIP, PHEXT_ENC_BR, PHEXT_ENC_ZSTD]
                .into_iter()
                .min_by_key(|&e| len(e))
                .unwrap();
            assert_eq!(pick("gzip, deflate, br, zstd"), smallest);
            assert_eq!(pick("gzip"), PHEXT_ENC_GZIP);
            assert_eq!(pick("gzip;q=0, br"), PHEXT_ENC_BR);
            assert_eq!(pick(""), PHEXT_ENC_IDENTITY);
            assert_eq!(
                phext_site_pick(site, std::ptr::null(), 0),
                PHEXT_ENC_IDENTITY
            );
            assert_eq!(pick("identity;q=0, deflate"), -1);
            assert_eq!(pick("*;q=0.5"), smallest);

            let mut tag = [0 as c_char; 32];
            assert_eq!(
                phext_site_etag(site, PHEXT_ENC_BR, tag.as_mut_ptr(), tag.len()),
                21
            );
            assert!(CStr::from_ptr(tag.as_ptr())
                .to_str()
                .unwrap()
                .ends_with("-br\""));

            // change detection
            let d = CString::new(dir.to_str().unwrap()).unwrap();
            let fp = phext_dir_fingerprint(d.as_ptr(), std::ptr::null());
            assert_eq!(fp, phext_site_fingerprint(site));
            std::fs::write(dir.join("css/a.css.gz"), b"still ignored").unwrap();
            assert_eq!(phext_dir_fingerprint(d.as_ptr(), std::ptr::null()), fp);
            std::fs::write(dir.join("new.js"), b"export {}").unwrap();
            assert_ne!(phext_dir_fingerprint(d.as_ptr(), std::ptr::null()), fp);
            phext_site_free(site);

            // errors come back as messages, not crashes
            let missing = CString::new(dir.join("nope").to_str().unwrap()).unwrap();
            let mut s = std::ptr::null_mut();
            let mut err = [0 as c_char; 256];
            assert_eq!(
                phext_site_build(
                    missing.as_ptr(),
                    std::ptr::null(),
                    &mut s,
                    err.as_mut_ptr(),
                    err.len()
                ),
                -1
            );
            assert!(CStr::from_ptr(err.as_ptr())
                .to_str()
                .unwrap()
                .starts_with("phext: "));
            assert_eq!(phext_dir_fingerprint(missing.as_ptr(), std::ptr::null()), 0);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
