//! Regenerates core/test-vectors from the phext-stream decoder (which tests/differential.rs pins to
//! libphext 0.3.1). Run: cargo run -p phext-stream --example gen_vectors

use phext_stream::{Coordinate, Decoder, Encoder, Entry, Event, Header, Location};
use std::fmt::Write as _;
use std::path::Path;

fn decode(input: &[u8]) -> Vec<(Coordinate, Vec<u8>)> {
    let mut out: Vec<(Coordinate, Vec<u8>)> = Vec::new();
    let mut dec = Decoder::new();
    let mut on = |ev: Event<'_>| match ev {
        Event::ScrollStart(c) => out.push((c, Vec::new())),
        Event::Data(d) => out.last_mut().unwrap().1.extend_from_slice(d),
        Event::ScrollEnd(_) => {}
    };
    dec.feed(input, &mut on);
    dec.finish(&mut on);
    out
}

fn json_string(s: &str) -> String {
    let mut o = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => write!(o, "\\u{:04x}", c as u32).unwrap(),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn write_vector(dir: &Path, name: &str, input: &[u8]) {
    let scrolls = decode(input);
    let mut json = String::from("[\n");
    for (i, (c, body)) in scrolls.iter().enumerate() {
        let text = std::str::from_utf8(body).expect("vectors are UTF-8 text");
        write!(
            json,
            "  {{\"coord\": \"{c}\", \"text\": {}}}",
            json_string(text)
        )
        .unwrap();
        json.push_str(if i + 1 < scrolls.len() { ",\n" } else { "\n" });
    }
    json.push_str("]\n");
    std::fs::write(dir.join(format!("{name}.phext")), input).unwrap();
    std::fs::write(dir.join(format!("{name}.expected.json")), json).unwrap();
    println!("{name}: {} bytes, {} scrolls", input.len(), scrolls.len());
}

struct Rng(u64);
impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n
    }
}

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-vectors");
    std::fs::create_dir_all(&dir).unwrap();

    let hand: &[(&str, &[u8])] = &[
        ("plain-text", b"just one scroll\nwith two lines"),
        (
            "every-dimension",
            b"sc1\x17sc2\x18sn2\x19ch2\x1Abk2\x1Cvm2\x1Dcn2\x1Esr2\x1Fsf2\x01lb2",
        ),
        (
            "leading-and-trailing-breaks",
            b"\x17\x17\x18first\x17\x17\x17second\x01\x1F",
        ),
        ("empty-scrolls-skipped", b"a\x17\x17\x17\x17b\x18\x18c"),
        (
            "reset-lower-dimensions",
            b"a\x17\x17\x17\x18b\x17\x17\x01c\x17d",
        ),
        (
            "unicode",
            "caf\u{e9}\x17\u{2192} arrows\x18\u{1D11E} clef\x19\u{4E2D}\u{6587}".as_bytes(),
        ),
        (
            "line-breaks-and-controls",
            b"line1\nline2\r\n\ttab\x07bell\x02stx\x1Besc\x17next",
        ),
        ("only-breaks", b"\x17\x18\x19\x1A\x1C\x1D\x1E\x1F\x01"),
        ("empty", b""),
    ];
    for (name, input) in hand {
        write_vector(&dir, name, input);
    }

    // saturation: libphext never advances a dimension to 1000
    let mut sat = b"start".to_vec();
    sat.extend(std::iter::repeat_n(0x17u8, 1003));
    sat.extend_from_slice(b"saturated");
    write_vector(&dir, "scroll-saturation", &sat);

    // the libphext regression corpus, if a sibling checkout exists
    let helios = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../libphext-rs/regression-1-helios.phext");
    if let Ok(bytes) = std::fs::read(&helios) {
        write_vector(&dir, "libphext-regression-1-helios", &bytes);
    }

    // a realistic bundle
    let header = Header {
        entries: vec![
            Entry {
                location: Location::Inline("1.1.1/1.1.1/1.1.2".parse().unwrap()),
                media_type: "text/html".into(),
                base64: false,
                path: "/index.html".into(),
            },
            Entry {
                location: Location::Inline("1.1.1/1.1.1/1.1.3".parse().unwrap()),
                media_type: "text/css".into(),
                base64: false,
                path: "/app.css".into(),
            },
            Entry {
                location: Location::Inline("1.1.1/1.1.1/1.2.1".parse().unwrap()),
                media_type: "image/gif".into(),
                base64: true,
                path: "/dot.gif".into(),
            },
            Entry {
                location: Location::External,
                media_type: "image/jpeg".into(),
                base64: false,
                path: "/hero.jpg".into(),
            },
        ],
    };
    let mut bundle = Vec::new();
    let mut enc = Encoder::new();
    enc.write_scroll(&mut bundle, Coordinate::ORIGIN, header.encode().as_bytes())
        .unwrap();
    enc.write_scroll(&mut bundle, "1.1.1/1.1.1/1.1.2".parse().unwrap(), b"<!doctype html><link rel=stylesheet href=/app.css><img src=/dot.gif><img src=/hero.jpg>\n").unwrap();
    enc.write_scroll(
        &mut bundle,
        "1.1.1/1.1.1/1.1.3".parse().unwrap(),
        b"img{display:block}\n",
    )
    .unwrap();
    enc.write_scroll(
        &mut bundle,
        "1.1.1/1.1.1/1.2.1".parse().unwrap(),
        b"R0lGODlhAQABAAAAACw=",
    )
    .unwrap();
    write_vector(&dir, "bundle-basic", &bundle);

    // random documents with fixed seeds
    for seed in 1..=8u64 {
        let mut rng = Rng(0xC0FFEE + seed);
        let mut s = String::new();
        let len = 50 + rng.below(1500) as usize;
        while s.len() < len {
            match rng.below(20) {
                0..=11 => s.push((b'a' + rng.below(26) as u8) as char),
                12 => s.push('\n'),
                13 => {
                    s.push(['\u{e9}', '\u{2192}', '\u{1D11E}', '\u{4E2D}'][rng.below(4) as usize])
                }
                14..=16 => s.push('\x17'),
                17 => s.push('\x18'),
                _ => s.push(
                    [0x01u8, 0x1F, 0x1E, 0x1D, 0x1C, 0x1A, 0x19][rng.below(7) as usize] as char,
                ),
            }
        }
        write_vector(&dir, &format!("random-{seed}"), s.as_bytes());
    }
}
