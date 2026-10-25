use crate::coord::{Break, Coordinate, CLASS};
use core::ops::Range;

/// What the decoder reports while walking a stream.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event<'a> {
    /// The first content byte for this coordinate has arrived.
    ScrollStart(Coordinate),
    /// Content for the scroll that is currently open. Borrowed from the chunk passed to `feed`; a scroll
    /// split across chunks arrives as several `Data` events.
    Data(&'a [u8]),
    /// The open scroll is complete (a break arrived, or `finish` was called).
    ScrollEnd(Coordinate),
}

/// Incremental phext decoder. Holds a coordinate and one flag; never buffers content.
#[derive(Clone, Debug, Default)]
pub struct Decoder {
    coord: Coordinate,
    open: bool,
    consumed: u64,
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder::default()
    }

    /// Coordinate the next content byte would land on.
    pub fn coordinate(&self) -> Coordinate {
        self.coord
    }

    /// Total bytes fed so far.
    pub fn consumed(&self) -> u64 {
        self.consumed
    }

    /// True while a scroll has started and not yet ended.
    pub fn in_scroll(&self) -> bool {
        self.open
    }

    pub fn feed<'a, F: FnMut(Event<'a>)>(&mut self, chunk: &'a [u8], mut sink: F) {
        let mut i = 0;
        let n = chunk.len();
        while i < n {
            let class = CLASS[chunk[i] as usize];
            if class == 0 {
                let start = i;
                i += 1;
                while i < n && CLASS[chunk[i] as usize] == 0 {
                    i += 1;
                }
                if !self.open {
                    self.open = true;
                    sink(Event::ScrollStart(self.coord));
                }
                sink(Event::Data(&chunk[start..i]));
            } else {
                if self.open {
                    self.open = false;
                    sink(Event::ScrollEnd(self.coord));
                }
                self.coord.apply(Break::for_dimension((class - 1) as usize));
                i += 1;
            }
        }
        self.consumed += n as u64;
    }

    /// Ends the stream, closing the open scroll if there is one.
    pub fn finish<F: FnMut(Event<'_>)>(&mut self, mut sink: F) {
        if self.open {
            self.open = false;
            sink(Event::ScrollEnd(self.coord));
        }
    }
}

/// Byte range of the scroll at `target` inside a complete buffer, if it exists. No allocation; stops
/// scanning as soon as the stream moves past `target`.
pub fn find(buf: &[u8], target: Coordinate) -> Option<Range<usize>> {
    let mut coord = Coordinate::ORIGIN;
    let mut start: Option<usize> = None;
    for (i, &b) in buf.iter().enumerate() {
        let class = CLASS[b as usize];
        if class == 0 {
            if coord == target && start.is_none() {
                start = Some(i);
            }
            continue;
        }
        if let Some(s) = start {
            return Some(s..i);
        }
        coord.apply(Break::for_dimension((class - 1) as usize));
        if coord > target {
            return None;
        }
    }
    start.map(|s| s..buf.len())
}
