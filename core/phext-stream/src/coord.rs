use core::fmt;

/// Number of phext dimensions above the line: library, shelf, series, collection, volume, book, chapter,
/// section, scroll.
pub const DIMENSIONS: usize = 9;

/// Highest index any dimension can reach. libphext 0.3.x refuses to advance a dimension to 1000, so a
/// break at 999 saturates; we match that so both decoders agree on every input.
pub const MAX_INDEX: u16 = 999;

/// A phext coordinate, ordered from the largest dimension (library) to the smallest (scroll).
///
/// The derived `Ord` is lexicographic over that order, which is exactly stream order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Coordinate(pub [u16; DIMENSIONS]);

impl Coordinate {
    /// `1.1.1/1.1.1/1.1.1`, where every stream starts.
    pub const ORIGIN: Coordinate = Coordinate([1; DIMENSIONS]);

    /// Applies a dimension break: advance that dimension (saturating at [`MAX_INDEX`]) and reset every
    /// smaller dimension to 1.
    #[inline]
    pub fn apply(&mut self, brk: Break) {
        let d = brk.dimension();
        if self.0[d] < MAX_INDEX {
            self.0[d] += 1;
        }
        let mut j = d + 1;
        while j < DIMENSIONS {
            self.0[j] = 1;
            j += 1;
        }
    }

    /// True when every dimension is within `1..=MAX_INDEX`.
    pub fn is_valid(&self) -> bool {
        self.0.iter().all(|&v| (1..=MAX_INDEX).contains(&v))
    }

    /// Parses `a.b.c/d.e.f/g.h.i` or the URL form `a.b.c;d.e.f;g.h.i`.
    pub fn parse(s: &str) -> Result<Coordinate, ParseError> {
        Self::parse_bytes(s.as_bytes())
    }

    pub fn parse_bytes(s: &[u8]) -> Result<Coordinate, ParseError> {
        let mut out = [0u16; DIMENSIONS];
        let mut dim = 0;
        let mut value: u32 = 0;
        let mut digits = 0;
        for (i, &b) in s.iter().enumerate() {
            match b {
                b'0'..=b'9' => {
                    value = value * 10 + (b - b'0') as u32;
                    digits += 1;
                    if value > MAX_INDEX as u32 {
                        return Err(ParseError::OutOfRange);
                    }
                }
                b'.' | b'/' | b';' => {
                    // groups of three: separators after positions 2 and 5 must be macro breaks
                    let macro_break = dim == 2 || dim == 5;
                    let ok = if macro_break {
                        b == b'/' || b == b';'
                    } else {
                        b == b'.'
                    };
                    if !ok || digits == 0 || dim + 1 >= DIMENSIONS {
                        return Err(ParseError::Syntax(i));
                    }
                    out[dim] = value as u16;
                    dim += 1;
                    value = 0;
                    digits = 0;
                }
                _ => return Err(ParseError::Syntax(i)),
            }
        }
        if digits == 0 || dim != DIMENSIONS - 1 {
            return Err(ParseError::Syntax(s.len()));
        }
        out[dim] = value as u16;
        if out.contains(&0) {
            return Err(ParseError::OutOfRange);
        }
        Ok(Coordinate(out))
    }

    /// Display adapter for the URL form (`;` between groups).
    pub fn url(&self) -> impl fmt::Display + '_ {
        Url(self)
    }

    fn write(&self, f: &mut fmt::Formatter<'_>, macro_sep: char) -> fmt::Result {
        let d = &self.0;
        write!(
            f,
            "{}.{}.{}{m}{}.{}.{}{m}{}.{}.{}",
            d[0],
            d[1],
            d[2],
            d[3],
            d[4],
            d[5],
            d[6],
            d[7],
            d[8],
            m = macro_sep
        )
    }
}

impl Default for Coordinate {
    fn default() -> Self {
        Coordinate::ORIGIN
    }
}

impl fmt::Display for Coordinate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write(f, '/')
    }
}

struct Url<'a>(&'a Coordinate);

impl fmt::Display for Url<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.write(f, ';')
    }
}

impl core::str::FromStr for Coordinate {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Coordinate::parse(s)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParseError {
    /// Malformed at this byte offset.
    Syntax(usize),
    /// A dimension was 0 or above [`MAX_INDEX`].
    OutOfRange,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Syntax(at) => write!(
                f,
                "malformed phext coordinate at byte {at}; expected LB.SF.SR/CN.VM.BK/CH.SN.SC"
            ),
            ParseError::OutOfRange => {
                write!(f, "phext coordinate dimension out of range 1..={MAX_INDEX}")
            }
        }
    }
}

/// The nine dimension breaks, as their wire bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Break {
    Library = 0x01,
    Shelf = 0x1F,
    Series = 0x1E,
    Collection = 0x1D,
    Volume = 0x1C,
    Book = 0x1A,
    Chapter = 0x19,
    Section = 0x18,
    Scroll = 0x17,
}

/// `CLASS[b]` is 0 for content bytes, otherwise the break's dimension + 1.
pub(crate) static CLASS: [u8; 256] = {
    let mut t = [0u8; 256];
    t[0x01] = 1;
    t[0x1F] = 2;
    t[0x1E] = 3;
    t[0x1D] = 4;
    t[0x1C] = 5;
    t[0x1A] = 6;
    t[0x19] = 7;
    t[0x18] = 8;
    t[0x17] = 9;
    t
};

const BY_DIMENSION: [Break; DIMENSIONS] = [
    Break::Library,
    Break::Shelf,
    Break::Series,
    Break::Collection,
    Break::Volume,
    Break::Book,
    Break::Chapter,
    Break::Section,
    Break::Scroll,
];

impl Break {
    #[inline]
    pub const fn from_byte(b: u8) -> Option<Break> {
        match CLASS[b as usize] {
            0 => None,
            c => Some(BY_DIMENSION[(c - 1) as usize]),
        }
    }

    #[inline]
    pub const fn is_break(b: u8) -> bool {
        CLASS[b as usize] != 0
    }

    /// Index into [`Coordinate`]'s array: 0 = library … 8 = scroll.
    #[inline]
    pub const fn dimension(self) -> usize {
        (CLASS[self as usize] - 1) as usize
    }

    #[inline]
    pub const fn for_dimension(d: usize) -> Break {
        BY_DIMENSION[d]
    }

    #[inline]
    pub const fn byte(self) -> u8 {
        self as u8
    }
}

impl core::error::Error for ParseError {}
