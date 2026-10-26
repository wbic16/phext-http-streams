use crate::coord::{Break, Coordinate, DIMENSIONS};
use core::fmt;

/// Tracks stream position on the writing side and emits the minimal break run between scrolls.
///
/// Usage: for each scroll in increasing coordinate order, write the bytes from [`Encoder::seek`], then the
/// scroll's content (which must contain no break bytes; see [`crate::validate`]).
#[derive(Clone, Debug, Default)]
pub struct Encoder {
    coord: Coordinate,
    started: bool,
}

impl Encoder {
    pub fn new() -> Encoder {
        Encoder::default()
    }

    pub fn coordinate(&self) -> Coordinate {
        self.coord
    }

    /// Moves to `next` and returns the break bytes to write before its content. `next` must be valid and
    /// strictly greater than the previous seek target (the first seek may target the origin).
    pub fn seek(&mut self, next: Coordinate) -> Result<Breaks, EncodeError> {
        if !next.is_valid() {
            return Err(EncodeError::Invalid(next));
        }
        if self.started && next <= self.coord {
            return Err(EncodeError::OutOfOrder {
                previous: self.coord,
                next,
            });
        }
        let breaks = Breaks {
            cur: self.coord,
            target: next,
        };
        self.coord = next;
        self.started = true;
        Ok(breaks)
    }

    /// Convenience: seek, validate, and append one whole scroll to `out`.
    #[cfg(feature = "alloc")]
    pub fn write_scroll(
        &mut self,
        out: &mut alloc::vec::Vec<u8>,
        at: Coordinate,
        content: &[u8],
    ) -> Result<(), EncodeError> {
        crate::validate(content).map_err(EncodeError::Forbidden)?;
        let breaks = self.seek(at)?;
        out.extend(breaks);
        out.extend_from_slice(content);
        Ok(())
    }
}

/// Iterator over the break bytes between two coordinates. At each step, break the highest dimension
/// where the current coordinate is still below the target.
#[derive(Clone, Debug)]
pub struct Breaks {
    cur: Coordinate,
    target: Coordinate,
}

impl Breaks {
    /// Number of bytes this run will produce, without consuming it.
    pub fn byte_len(&self) -> usize {
        self.clone().count()
    }
}

impl Iterator for Breaks {
    type Item = u8;

    #[inline]
    fn next(&mut self) -> Option<u8> {
        let d = (0..DIMENSIONS).find(|&d| self.cur.0[d] != self.target.0[d])?;
        let brk = Break::for_dimension(d);
        self.cur.apply(brk);
        Some(brk.byte())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EncodeError {
    Invalid(Coordinate),
    OutOfOrder {
        previous: Coordinate,
        next: Coordinate,
    },
    Forbidden(crate::ForbiddenByte),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodeError::Invalid(c) => write!(
                f,
                "coordinate {c} has a dimension outside 1..={}",
                crate::MAX_INDEX
            ),
            EncodeError::OutOfOrder { previous, next } => {
                write!(f, "scroll {next} does not come after {previous}")
            }
            EncodeError::Forbidden(e) => write!(f, "{e}"),
        }
    }
}

impl core::error::Error for EncodeError {}
