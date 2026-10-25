//! Zero-copy streaming phext.
//!
//! The wire format is described in `spec/bundle-v1.md`. In short: a phext stream is plain bytes where nine
//! ASCII control codes act as dimension breaks. A decoder only needs the current coordinate to place every
//! byte, so this crate never buffers content: [`Decoder::feed`] hands out borrowed slices of the input
//! chunk, and [`Encoder`] emits the minimal break run between two coordinates.
//!
//! The core types work in `no_std` without an allocator. The `alloc` feature (on by default) adds the
//! bundle header, the streaming [`BundleReader`], base64, and the scroll index.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;

mod coord;
mod decoder;
mod encoder;
mod validate;

#[cfg(feature = "alloc")]
pub mod base64;
#[cfg(feature = "alloc")]
mod bundle;
#[cfg(feature = "alloc")]
mod header;
#[cfg(feature = "alloc")]
mod index;

pub use coord::{Break, Coordinate, ParseError, DIMENSIONS, MAX_INDEX};
pub use decoder::{find, Decoder, Event};
pub use encoder::{Breaks, EncodeError, Encoder};
pub use validate::{validate, ForbiddenByte};

#[cfg(feature = "alloc")]
pub use bundle::{BundleError, BundleEvent, BundleReader};
#[cfg(feature = "alloc")]
pub use header::{Entry, Header, HeaderError, Location, MAGIC};
#[cfg(feature = "alloc")]
pub use index::{index, ScrollSpan};
