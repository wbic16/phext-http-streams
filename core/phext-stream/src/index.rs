use crate::coord::Coordinate;
use crate::decoder::{Decoder, Event};
use alloc::vec::Vec;

/// Where one scroll lives in a complete buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScrollSpan {
    pub coord: Coordinate,
    pub start: usize,
    pub end: usize,
}

/// One pass over `buf`, returning every non-empty scroll's byte range. This is what a server caches per
/// file to answer coordinate-addressed requests without rescanning.
pub fn index(buf: &[u8]) -> Vec<ScrollSpan> {
    let mut out: Vec<ScrollSpan> = Vec::new();
    let base = buf.as_ptr() as usize;
    let mut dec = Decoder::new();
    let mut on = |ev: Event<'_>| match ev {
        Event::ScrollStart(coord) => out.push(ScrollSpan {
            coord,
            start: 0,
            end: 0,
        }),
        Event::Data(d) => {
            // a single feed means each scroll is exactly one Data slice of `buf`
            if let Some(last) = out.last_mut() {
                last.start = d.as_ptr() as usize - base;
                last.end = last.start + d.len();
            }
        }
        Event::ScrollEnd(_) => {}
    };
    dec.feed(buf, &mut on);
    dec.finish(&mut on);
    out
}
