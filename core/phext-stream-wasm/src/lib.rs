//! WebAssembly surface for the bundle reader, without wasm-bindgen: plain exports plus a handful of
//! imported callbacks from the `phext` module. JS glue lives in `browser/sw-loader/phext-wasm.js`.
//!
//! Build: cargo build --release --target wasm32-unknown-unknown -p phext-stream-wasm
//!
//! Callbacks (imported from module "phext"):
//!   header(ptr, len)        normalized header text (UTF-8); arrives before any resource event
//!   start(index) / end(index)
//!   data(index, ptr, len)   resource bytes, already base64-decoded; valid only during the call
//!   unlisted(ptr, len)      coordinate text of a scroll the header doesn't list
//!   missing(index)          from finish(): listed inline but never arrived
//!   error(ptr, len)         first error message; the reader stops after it
//!
//! On non-wasm targets this crate is empty, so the workspace still builds everywhere.

#![cfg(target_arch = "wasm32")]

use phext_stream::{BundleEvent, BundleReader};

#[link(wasm_import_module = "phext")]
extern "C" {
    fn header(ptr: *const u8, len: usize);
    fn start(index: u32);
    fn data(index: u32, ptr: *const u8, len: usize);
    fn end(index: u32);
    fn unlisted(ptr: *const u8, len: usize);
    fn missing(index: u32);
    fn error(ptr: *const u8, len: usize);
}

fn emit(ev: BundleEvent<'_>) {
    // SAFETY: the imports only read the given ranges during the call.
    unsafe {
        match ev {
            BundleEvent::Header(h) => {
                let t = h.encode();
                header(t.as_ptr(), t.len());
            }
            BundleEvent::ResourceStart(i) => start(i as u32),
            BundleEvent::ResourceData(i, d) => data(i as u32, d.as_ptr(), d.len()),
            BundleEvent::ResourceEnd(i) => end(i as u32),
            BundleEvent::Unlisted(c) => {
                let t = c.to_string();
                unlisted(t.as_ptr(), t.len());
            }
        }
    }
}

/// The reader plus whether its (sticky) error has already gone out through `error`.
pub struct Reader {
    inner: BundleReader,
    reported: bool,
}

fn report(r: &mut Reader, e: &phext_stream::BundleError) {
    if std::mem::replace(&mut r.reported, true) {
        return;
    }
    let t = e.to_string();
    // SAFETY: as above
    unsafe { error(t.as_ptr(), t.len()) }
}

/// Scratch space for JS to copy chunks into.
#[no_mangle]
pub extern "C" fn phext_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// `ptr`/`len` must come from `phext_alloc`.
#[no_mangle]
pub unsafe extern "C" fn phext_free(ptr: *mut u8, len: usize) {
    drop(Vec::from_raw_parts(ptr, 0, len.max(1)));
}

#[no_mangle]
pub extern "C" fn phext_reader_new() -> *mut Reader {
    Box::into_raw(Box::new(Reader {
        inner: BundleReader::new(),
        reported: false,
    }))
}

/// # Safety
/// `r` must come from `phext_reader_new` and not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn phext_reader_free(r: *mut Reader) {
    if !r.is_null() {
        drop(Box::from_raw(r));
    }
}

/// Returns 0, or -1 after reporting an error through the `error` import.
///
/// # Safety
/// `r` from `phext_reader_new`; `ptr`/`len` a readable range.
#[no_mangle]
pub unsafe extern "C" fn phext_reader_feed(r: *mut Reader, ptr: *const u8, len: usize) -> i32 {
    let Some(r) = r.as_mut() else { return -1 };
    let chunk = if len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(ptr, len)
    };
    match r.inner.feed(chunk, emit) {
        Ok(()) => 0,
        Err(e) => {
            report(r, &e);
            -1
        }
    }
}

/// Ends the stream; reports `missing` entries. Returns 0, or -1 after reporting an error.
///
/// # Safety
/// `r` from `phext_reader_new`.
#[no_mangle]
pub unsafe extern "C" fn phext_reader_finish(r: *mut Reader) -> i32 {
    let Some(r) = r.as_mut() else { return -1 };
    match r.inner.finish(emit) {
        Ok(gone) => {
            for i in gone {
                missing(i as u32);
            }
            0
        }
        Err(e) => {
            report(r, &e);
            -1
        }
    }
}
