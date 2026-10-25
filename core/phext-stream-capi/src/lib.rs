//! C ABI over phext-stream. The matching header is `include/phext_stream.h`; keep the two in sync.
//!
//! Conventions: every pointer argument may be NULL (the call then does nothing and returns an error
//! value where it has one); lengths are byte counts; nothing here panics on any input.

#![allow(clippy::missing_safety_doc, non_camel_case_types)]

use core::ffi::{c_char, c_int, c_void};
use phext_stream::{Coordinate, Decoder, Encoder, Event, DIMENSIONS};
use std::io::Write;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct phext_coord {
    /// library, shelf, series, collection, volume, book, chapter, section, scroll
    pub d: [u16; DIMENSIONS],
}

impl From<Coordinate> for phext_coord {
    fn from(c: Coordinate) -> Self {
        phext_coord { d: c.0 }
    }
}

impl From<phext_coord> for Coordinate {
    fn from(c: phext_coord) -> Self {
        Coordinate(c.d)
    }
}

pub const PHEXT_SCROLL_START: c_int = 1;
pub const PHEXT_DATA: c_int = 2;
pub const PHEXT_SCROLL_END: c_int = 3;

/// `kind` is one of PHEXT_SCROLL_START / PHEXT_DATA / PHEXT_SCROLL_END. `coord` is set for start/end;
/// `data`/`len` are set for data and point into the caller's chunk.
pub type phext_event_cb = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        kind: c_int,
        coord: *const phext_coord,
        data: *const u8,
        len: usize,
    ),
>;

pub struct phext_decoder(Decoder);
pub struct phext_encoder(Encoder);

unsafe fn slice<'a>(data: *const u8, len: usize) -> &'a [u8] {
    if data.is_null() || len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(data, len)
    }
}

unsafe fn dispatch(cb: phext_event_cb, user: *mut c_void, ev: Event<'_>) {
    let Some(cb) = cb else { return };
    match ev {
        Event::ScrollStart(c) => {
            let pc = phext_coord::from(c);
            cb(user, PHEXT_SCROLL_START, &pc, std::ptr::null(), 0)
        }
        Event::Data(d) => cb(user, PHEXT_DATA, std::ptr::null(), d.as_ptr(), d.len()),
        Event::ScrollEnd(c) => {
            let pc = phext_coord::from(c);
            cb(user, PHEXT_SCROLL_END, &pc, std::ptr::null(), 0)
        }
    }
}

#[no_mangle]
pub extern "C" fn phext_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

#[no_mangle]
pub unsafe extern "C" fn phext_coord_origin(out: *mut phext_coord) {
    if let Some(out) = out.as_mut() {
        *out = Coordinate::ORIGIN.into();
    }
}

/// Parses `a.b.c/d.e.f/g.h.i` or `a.b.c;d.e.f;g.h.i` (not NUL-terminated; `len` bytes). Returns 0 on
/// success, -1 on error.
#[no_mangle]
pub unsafe extern "C" fn phext_coord_parse(
    s: *const c_char,
    len: usize,
    out: *mut phext_coord,
) -> c_int {
    let Some(out) = out.as_mut() else { return -1 };
    match Coordinate::parse_bytes(slice(s as *const u8, len)) {
        Ok(c) => {
            *out = c.into();
            0
        }
        Err(_) => -1,
    }
}

/// Writes the coordinate (`url` != 0 selects the `;` form) and a NUL into `buf` if it fits. Returns the
/// length excluding the NUL, like snprintf; at most 39 bytes, so a 40-byte buffer always suffices.
#[no_mangle]
pub unsafe extern "C" fn phext_coord_format(
    c: *const phext_coord,
    url: c_int,
    buf: *mut c_char,
    cap: usize,
) -> usize {
    let Some(c) = c.as_ref() else { return 0 };
    let coord = Coordinate::from(*c);
    let mut tmp = [0u8; 64];
    let mut cursor = std::io::Cursor::new(&mut tmp[..]);
    let _ = if url != 0 {
        write!(cursor, "{}", coord.url())
    } else {
        write!(cursor, "{coord}")
    };
    let n = cursor.position() as usize;
    if !buf.is_null() && n < cap {
        std::ptr::copy_nonoverlapping(tmp.as_ptr(), buf as *mut u8, n);
        *buf.add(n) = 0;
    }
    n
}

/// Compares two coordinates in stream order: <0, 0, >0.
#[no_mangle]
pub unsafe extern "C" fn phext_coord_cmp(a: *const phext_coord, b: *const phext_coord) -> c_int {
    let (Some(a), Some(b)) = (a.as_ref(), b.as_ref()) else {
        return 0;
    };
    Coordinate::from(*a).cmp(&Coordinate::from(*b)) as c_int
}

#[no_mangle]
pub extern "C" fn phext_decoder_new() -> *mut phext_decoder {
    Box::into_raw(Box::new(phext_decoder(Decoder::new())))
}

#[no_mangle]
pub unsafe extern "C" fn phext_decoder_free(dec: *mut phext_decoder) {
    if !dec.is_null() {
        drop(Box::from_raw(dec));
    }
}

#[no_mangle]
pub unsafe extern "C" fn phext_decoder_feed(
    dec: *mut phext_decoder,
    data: *const u8,
    len: usize,
    cb: phext_event_cb,
    user: *mut c_void,
) {
    let Some(dec) = dec.as_mut() else { return };
    dec.0.feed(slice(data, len), |ev| dispatch(cb, user, ev));
}

#[no_mangle]
pub unsafe extern "C" fn phext_decoder_finish(
    dec: *mut phext_decoder,
    cb: phext_event_cb,
    user: *mut c_void,
) {
    let Some(dec) = dec.as_mut() else { return };
    dec.0.finish(|ev| dispatch(cb, user, ev));
}

#[no_mangle]
pub unsafe extern "C" fn phext_decoder_coord(dec: *const phext_decoder, out: *mut phext_coord) {
    if let (Some(dec), Some(out)) = (dec.as_ref(), out.as_mut()) {
        *out = dec.0.coordinate().into();
    }
}

#[no_mangle]
pub extern "C" fn phext_encoder_new() -> *mut phext_encoder {
    Box::into_raw(Box::new(phext_encoder(Encoder::new())))
}

#[no_mangle]
pub unsafe extern "C" fn phext_encoder_free(enc: *mut phext_encoder) {
    if !enc.is_null() {
        drop(Box::from_raw(enc));
    }
}

/// Computes the break bytes that move the stream to `next`. Returns their count. If that is <= `cap` the
/// bytes are written to `out` and the encoder advances; otherwise nothing changes and the caller retries
/// with a buffer of the returned size. Returns -1 if `next` is invalid or not after the previous target.
#[no_mangle]
pub unsafe extern "C" fn phext_encoder_seek(
    enc: *mut phext_encoder,
    next: *const phext_coord,
    out: *mut u8,
    cap: usize,
) -> i64 {
    let (Some(enc), Some(next)) = (enc.as_mut(), next.as_ref()) else {
        return -1;
    };
    let mut trial = enc.0.clone();
    let Ok(breaks) = trial.seek((*next).into()) else {
        return -1;
    };
    let n = breaks.byte_len();
    if n <= cap && (n == 0 || !out.is_null()) {
        for (i, b) in breaks.enumerate() {
            *out.add(i) = b;
        }
        enc.0 = trial;
    }
    n as i64
}

/// Returns 0 if `data` contains no break bytes; otherwise -1 and the first offending offset in
/// `*bad_offset` (if non-NULL).
#[no_mangle]
pub unsafe extern "C" fn phext_validate(
    data: *const u8,
    len: usize,
    bad_offset: *mut usize,
) -> c_int {
    match phext_stream::validate(slice(data, len)) {
        Ok(()) => 0,
        Err(e) => {
            if let Some(o) = bad_offset.as_mut() {
                *o = e.offset;
            }
            -1
        }
    }
}

/// Finds the scroll at `target` in a complete buffer. Returns 0 and its [start, end) byte range, or -1
/// if there is no content at that coordinate.
#[no_mangle]
pub unsafe extern "C" fn phext_find(
    data: *const u8,
    len: usize,
    target: *const phext_coord,
    start: *mut usize,
    end: *mut usize,
) -> c_int {
    let Some(target) = target.as_ref() else {
        return -1;
    };
    match phext_stream::find(slice(data, len), (*target).into()) {
        Some(r) => {
            if let Some(s) = start.as_mut() {
                *s = r.start;
            }
            if let Some(e) = end.as_mut() {
                *e = r.end;
            }
            0
        }
        None => -1,
    }
}
