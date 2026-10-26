//! Standard base64 (RFC 4648 §4, padded) for small binary scrolls, with a streaming decoder so binary
//! resources can be handed out while their scroll is still arriving.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

const INVALID: u8 = 0xFF;
const PAD: u8 = 0xFE;

static DECODE: [u8; 256] = {
    let mut t = [INVALID; 256];
    let mut i = 0;
    while i < 64 {
        t[ALPHABET[i] as usize] = i as u8;
        i += 1;
    }
    t[b'=' as usize] = PAD;
    t
};

/// Length of the padded encoding of `n` bytes.
pub const fn encoded_len(n: usize) -> usize {
    n.div_ceil(3) * 4
}

pub fn encode(input: &[u8]) -> String {
    let mut out = Vec::with_capacity(encoded_len(input.len()));
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63]);
        out.push(ALPHABET[(n >> 12) as usize & 63]);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63]
        } else {
            b'='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63]
        } else {
            b'='
        });
    }
    // Every byte pushed comes from ALPHABET or is '='.
    String::from_utf8(out).unwrap_or_default()
}

pub fn decode(input: &[u8]) -> Result<Vec<u8>, Base64Error> {
    let mut d = Decoder::new();
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    d.feed(input, &mut out)?;
    d.finish()?;
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Base64Error {
    /// Byte outside the alphabet, or data after padding, at this offset into the stream.
    InvalidByte(u64),
    /// Stream ended mid-quantum.
    Truncated,
}

impl fmt::Display for Base64Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Base64Error::InvalidByte(at) => write!(f, "invalid base64 at offset {at}"),
            Base64Error::Truncated => write!(f, "base64 ended mid-quantum"),
        }
    }
}

/// Incremental decoder: carries at most three symbols between `feed` calls.
#[derive(Clone, Debug, Default)]
pub struct Decoder {
    quad: [u8; 4],
    have: usize,
    pos: u64,
    done: bool,
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder::default()
    }

    pub fn feed(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<(), Base64Error> {
        for &c in input {
            let v = DECODE[c as usize];
            if v == INVALID || self.done {
                return Err(Base64Error::InvalidByte(self.pos));
            }
            self.quad[self.have] = v;
            self.have += 1;
            self.pos += 1;
            if self.have == 4 {
                self.flush_quad(out)?;
            }
        }
        Ok(())
    }

    fn flush_quad(&mut self, out: &mut Vec<u8>) -> Result<(), Base64Error> {
        let q = self.quad;
        self.have = 0;
        let bad = Base64Error::InvalidByte(self.pos - 4);
        if q[0] == PAD || q[1] == PAD || (q[2] == PAD && q[3] != PAD) {
            return Err(bad);
        }
        let n = (q[0] as u32) << 18 | (q[1] as u32) << 12;
        out.push((n >> 16) as u8);
        if q[2] == PAD {
            self.done = true;
            return Ok(());
        }
        let n = n | (q[2] as u32) << 6;
        out.push((n >> 8) as u8);
        if q[3] == PAD {
            self.done = true;
            return Ok(());
        }
        out.push((n | q[3] as u32) as u8);
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), Base64Error> {
        if self.have != 0 {
            return Err(Base64Error::Truncated);
        }
        Ok(())
    }
}

impl core::error::Error for Base64Error {}
