#![allow(dead_code)]

use phext_stream::{Coordinate, Decoder, Event};

/// Tiny deterministic PRNG so the randomized tests need no dependencies and reproduce from a seed.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

pub const BREAKS: [u8; 9] = [0x01, 0x1F, 0x1E, 0x1D, 0x1C, 0x1A, 0x19, 0x18, 0x17];

/// Random phext text: mostly content (ASCII, newlines, multi-byte UTF-8), with breaks weighted toward
/// the small dimensions the way real documents are.
pub fn random_phext(rng: &mut Rng, len: usize) -> String {
    let mut s = String::new();
    while s.len() < len {
        match rng.below(20) {
            0..=10 => s.push((b'a' + rng.below(26) as u8) as char),
            11 => s.push('\n'),
            12 => s.push(['é', '→', '𝄞', '中'][rng.below(4) as usize]),
            13..=15 => s.push(0x17 as char),
            16 => s.push(0x18 as char),
            17 => s.push(0x19 as char),
            _ => s.push(BREAKS[rng.below(9) as usize] as char),
        }
    }
    s
}

/// Decodes with the given chunk sizes (cycled) and returns (coordinate, content) per scroll.
pub fn decode_chunked(input: &[u8], chunks: &[usize]) -> Vec<(Coordinate, Vec<u8>)> {
    let mut out: Vec<(Coordinate, Vec<u8>)> = Vec::new();
    let mut ended = 0;
    let mut dec = Decoder::new();
    let mut on = |ev: Event<'_>| match ev {
        Event::ScrollStart(c) => out.push((c, Vec::new())),
        Event::Data(d) => out
            .last_mut()
            .expect("data before start")
            .1
            .extend_from_slice(d),
        Event::ScrollEnd(c) => {
            assert_eq!(out.last().expect("end before start").0, c);
            ended += 1;
        }
    };
    let mut pos = 0;
    let mut k = 0;
    while pos < input.len() {
        let n = chunks[k % chunks.len()].max(1).min(input.len() - pos);
        dec.feed(&input[pos..pos + n], &mut on);
        pos += n;
        k += 1;
    }
    dec.finish(&mut on);
    assert_eq!(ended, out.len(), "every started scroll must end");
    out
}

pub fn c(s: &str) -> Coordinate {
    Coordinate::parse(s).unwrap()
}
