//! Streaming bundle reader: phext stream in, per-resource events out (spec §3).

use crate::base64;
use crate::coord::Coordinate;
use crate::decoder::{Decoder, Event};
use crate::header::{Header, HeaderError, Location};
use alloc::vec::Vec;
use core::fmt;

/// Headers larger than this are rejected rather than buffered without bound.
const DEFAULT_MAX_HEADER: usize = 64 * 1024;

#[derive(Debug)]
pub enum BundleEvent<'a> {
    /// The header scroll is complete. Arrives before any resource event; external entries can be fetched
    /// from here on.
    Header(&'a Header),
    /// `index` into `header.entries`.
    ResourceStart(usize),
    /// Resource bytes, already base64-decoded for `;base64` entries.
    ResourceData(usize, &'a [u8]),
    ResourceEnd(usize),
    /// A scroll at a coordinate the header doesn't list. Its content is skipped.
    Unlisted(Coordinate),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BundleError {
    /// The stream's first scroll is not at the origin.
    MissingHeader(Coordinate),
    HeaderTooLarge,
    Header(HeaderError),
    Base64 {
        index: usize,
        error: base64::Base64Error,
    },
    /// The same listed coordinate appeared twice (possible only through saturated breaks).
    Repeated(usize),
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BundleError::MissingHeader(c) => write!(
                f,
                "bundle starts at {c}, expected the header at 1.1.1/1.1.1/1.1.1"
            ),
            BundleError::HeaderTooLarge => write!(f, "bundle header exceeds the size limit"),
            BundleError::Header(e) => write!(f, "{e}"),
            BundleError::Base64 { index, error } => write!(f, "resource {index}: {error}"),
            BundleError::Repeated(i) => write!(f, "resource {i} appeared twice"),
        }
    }
}

enum State {
    Header,
    Body,
    Failed,
}

struct Inner {
    state: State,
    header_buf: Vec<u8>,
    max_header: usize,
    header: Header,
    seen: Vec<bool>,
    current: Option<usize>,
    b64: base64::Decoder,
    scratch: Vec<u8>,
    error: Option<BundleError>,
}

/// Feed it the decompressed response body chunk by chunk; it emits [`BundleEvent`]s as soon as each
/// piece is known. Holds at most the header and one base64 scratch buffer.
pub struct BundleReader {
    dec: Decoder,
    inner: Inner,
}

impl Default for BundleReader {
    fn default() -> Self {
        Self::new()
    }
}

impl BundleReader {
    pub fn new() -> BundleReader {
        Self::with_max_header(DEFAULT_MAX_HEADER)
    }

    pub fn with_max_header(max_header: usize) -> BundleReader {
        BundleReader {
            dec: Decoder::new(),
            inner: Inner {
                state: State::Header,
                header_buf: Vec::new(),
                max_header,
                header: Header::default(),
                seen: Vec::new(),
                current: None,
                b64: base64::Decoder::new(),
                scratch: Vec::new(),
                error: None,
            },
        }
    }

    /// Parsed header; empty until [`BundleEvent::Header`] has been emitted.
    pub fn header(&self) -> &Header {
        &self.inner.header
    }

    /// After a failure the reader ignores further input and keeps returning the first error.
    pub fn feed<F: FnMut(BundleEvent<'_>)>(
        &mut self,
        chunk: &[u8],
        mut sink: F,
    ) -> Result<(), BundleError> {
        let inner = &mut self.inner;
        if let Some(e) = &inner.error {
            return Err(e.clone());
        }
        self.dec.feed(chunk, |ev| inner.on(ev, &mut sink));
        inner.error.clone().map_or(Ok(()), Err)
    }

    /// Ends the stream. Returns the listed inline entries that never arrived (clients should fetch those
    /// paths normally).
    pub fn finish<F: FnMut(BundleEvent<'_>)>(
        &mut self,
        mut sink: F,
    ) -> Result<Vec<usize>, BundleError> {
        let inner = &mut self.inner;
        if let Some(e) = &inner.error {
            return Err(e.clone());
        }
        self.dec.finish(|ev| inner.on(ev, &mut sink));
        if let Some(e) = &inner.error {
            return Err(e.clone());
        }
        if matches!(inner.state, State::Header) {
            // empty stream, or the header never closed
            return Err(BundleError::MissingHeader(self.dec.coordinate()));
        }
        Ok(inner
            .header
            .entries
            .iter()
            .enumerate()
            .filter(|(i, e)| matches!(e.location, Location::Inline(_)) && !inner.seen[*i])
            .map(|(i, _)| i)
            .collect())
    }
}

impl Inner {
    fn fail(&mut self, e: BundleError) {
        self.state = State::Failed;
        self.error = Some(e);
    }

    fn on<F: FnMut(BundleEvent<'_>)>(&mut self, ev: Event<'_>, sink: &mut F) {
        match self.state {
            State::Failed => {}
            State::Header => match ev {
                Event::ScrollStart(c) if c != Coordinate::ORIGIN => {
                    self.fail(BundleError::MissingHeader(c))
                }
                Event::ScrollStart(_) => {}
                Event::Data(d) => {
                    if self.header_buf.len() + d.len() > self.max_header {
                        self.fail(BundleError::HeaderTooLarge);
                    } else {
                        self.header_buf.extend_from_slice(d);
                    }
                }
                Event::ScrollEnd(_) => match Header::parse(&self.header_buf) {
                    Ok(h) => {
                        self.seen = alloc::vec![false; h.entries.len()];
                        self.header = h;
                        self.header_buf = Vec::new();
                        self.state = State::Body;
                        sink(BundleEvent::Header(&self.header));
                    }
                    Err(e) => self.fail(BundleError::Header(e)),
                },
            },
            State::Body => match ev {
                Event::ScrollStart(c) => match self.header.entry_at(c) {
                    Some((i, _)) if self.seen[i] => self.fail(BundleError::Repeated(i)),
                    Some((i, e)) => {
                        self.seen[i] = true;
                        self.current = Some(i);
                        if e.base64 {
                            self.b64 = base64::Decoder::new();
                        }
                        sink(BundleEvent::ResourceStart(i));
                    }
                    None => {
                        self.current = None;
                        sink(BundleEvent::Unlisted(c));
                    }
                },
                Event::Data(d) => {
                    let Some(i) = self.current else { return };
                    if self.header.entries[i].base64 {
                        self.scratch.clear();
                        match self.b64.feed(d, &mut self.scratch) {
                            Ok(()) if !self.scratch.is_empty() => {
                                sink(BundleEvent::ResourceData(i, &self.scratch))
                            }
                            Ok(()) => {}
                            Err(error) => self.fail(BundleError::Base64 { index: i, error }),
                        }
                    } else {
                        sink(BundleEvent::ResourceData(i, d));
                    }
                }
                Event::ScrollEnd(_) => {
                    let Some(i) = self.current.take() else { return };
                    if self.header.entries[i].base64 {
                        if let Err(error) = self.b64.finish() {
                            return self.fail(BundleError::Base64 { index: i, error });
                        }
                    }
                    sink(BundleEvent::ResourceEnd(i));
                }
            },
        }
    }
}
