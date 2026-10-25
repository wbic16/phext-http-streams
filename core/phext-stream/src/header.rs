//! The bundle header scroll at `1.1.1/1.1.1/1.1.1` (spec §3.1).

use crate::coord::{Coordinate, ParseError};
use alloc::collections::BTreeSet;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::{self, Write};

/// First line of every bundle header.
pub const MAGIC: &str = "phext-bundle 1";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Location {
    /// Carried in the bundle at this coordinate.
    Inline(Coordinate),
    /// Fetched by the client over normal HTTP.
    External,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub location: Location,
    /// Media type without the `;base64` flag.
    pub media_type: String,
    pub base64: bool,
    /// Absolute URL path, starting with `/`.
    pub path: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Header {
    /// In priority order.
    pub entries: Vec<Entry>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HeaderError {
    NotUtf8,
    /// First line was not [`MAGIC`].
    Magic,
    /// 1-based line number and what was wrong with it.
    Line(usize, &'static str),
    Coordinate(usize, ParseError),
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeaderError::NotUtf8 => write!(f, "bundle header is not UTF-8"),
            HeaderError::Magic => write!(f, "bundle header must start with `{MAGIC}`"),
            HeaderError::Line(n, why) => write!(f, "bundle header line {n}: {why}"),
            HeaderError::Coordinate(n, e) => write!(f, "bundle header line {n}: {e}"),
        }
    }
}

impl Header {
    pub fn parse(bytes: &[u8]) -> Result<Header, HeaderError> {
        let text = core::str::from_utf8(bytes).map_err(|_| HeaderError::NotUtf8)?;
        let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
        if lines.next() != Some(MAGIC) {
            return Err(HeaderError::Magic);
        }
        let mut entries = Vec::new();
        let mut coords = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for (i, line) in lines.enumerate() {
            let n = i + 2;
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let mut fields = line.split_ascii_whitespace();
            let (Some(loc), Some(media), Some(path), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err(HeaderError::Line(
                    n,
                    "expected `<coordinate|external> <media-type> <path>`",
                ));
            };
            let location = if loc == "external" {
                Location::External
            } else {
                let c = Coordinate::parse(loc).map_err(|e| HeaderError::Coordinate(n, e))?;
                if c == Coordinate::ORIGIN {
                    return Err(HeaderError::Line(
                        n,
                        "1.1.1/1.1.1/1.1.1 is reserved for the header",
                    ));
                }
                if !coords.insert(c) {
                    return Err(HeaderError::Line(n, "coordinate listed twice"));
                }
                Location::Inline(c)
            };
            let (media_type, base64) = match media.strip_suffix(";base64") {
                Some(m) => (m, true),
                None => (media, false),
            };
            if media_type.is_empty() {
                return Err(HeaderError::Line(n, "empty media type"));
            }
            if base64 && location == Location::External {
                return Err(HeaderError::Line(
                    n,
                    ";base64 only applies to inline entries",
                ));
            }
            if !path.starts_with('/') {
                return Err(HeaderError::Line(n, "path must start with /"));
            }
            if !paths.insert(path) {
                return Err(HeaderError::Line(n, "path listed twice"));
            }
            entries.push(Entry {
                location,
                media_type: media_type.to_string(),
                base64,
                path: path.to_string(),
            });
        }
        Ok(Header { entries })
    }

    /// Serializes to the header scroll's content. The result never contains break bytes as long as the
    /// entries' strings don't (paths and media types are expected to be URL-safe ASCII).
    pub fn encode(&self) -> String {
        let mut s = String::from(MAGIC);
        for e in &self.entries {
            s.push('\n');
            match e.location {
                Location::Inline(c) => write!(s, "{c}"),
                Location::External => write!(s, "external"),
            }
            .ok();
            write!(
                s,
                " {}{} {}",
                e.media_type,
                if e.base64 { ";base64" } else { "" },
                e.path
            )
            .ok();
        }
        s.push('\n');
        s
    }

    pub fn entry_at(&self, c: Coordinate) -> Option<(usize, &Entry)> {
        self.entries
            .iter()
            .enumerate()
            .find(|(_, e)| e.location == Location::Inline(c))
    }
}
