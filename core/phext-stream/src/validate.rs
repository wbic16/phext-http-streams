use crate::coord::Break;
use core::fmt;

/// A content byte that would be read as a dimension break.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ForbiddenByte {
    pub offset: usize,
    pub byte: u8,
}

impl fmt::Display for ForbiddenByte {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "byte 0x{:02X} at offset {} is a phext dimension break",
            self.byte, self.offset
        )
    }
}

/// Checks that `content` can travel as one unescaped scroll.
pub fn validate(content: &[u8]) -> Result<(), ForbiddenByte> {
    match content.iter().position(|&b| Break::is_break(b)) {
        None => Ok(()),
        Some(offset) => Err(ForbiddenByte {
            offset,
            byte: content[offset],
        }),
    }
}
