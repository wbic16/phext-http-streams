//! A dependency-free stand-in for fuzz/fuzz_targets/decode.rs, so the same properties run under plain
//! `cargo test` on every platform. Inputs are arbitrary bytes, including invalid UTF-8.

mod common;

use common::{decode_chunked, Rng, BREAKS};
use libphext::phext as reference;
use phext_stream::{find, index, BundleReader, Coordinate, Encoder};

fn random_bytes(rng: &mut Rng, len: usize) -> Vec<u8> {
    (0..len)
        .map(|_| match rng.below(4) {
            0 => BREAKS[rng.below(9) as usize],
            _ => rng.next() as u8,
        })
        .collect()
}

#[test]
fn arbitrary_bytes_hold_all_properties() {
    let mut rng = Rng(0xF022_0001);
    let mut utf8_cases = 0;
    for _ in 0..20_000 {
        let len = rng.below(256) as usize;
        let mut input = random_bytes(&mut rng, len);
        if rng.below(3) == 0 {
            // keep a share of valid UTF-8 so the libphext comparisons run often
            input.retain(|b| b.is_ascii());
        }
        let split: Vec<usize> = (0..1 + rng.below(4))
            .map(|_| 1 + rng.below(9) as usize)
            .collect();

        let whole = decode_chunked(&input, &[usize::MAX]);
        assert_eq!(
            decode_chunked(&input, &split),
            whole,
            "chunking changed the result"
        );

        let spans = index(&input);
        assert_eq!(spans.len(), whole.len());
        for (s, (c, body)) in spans.iter().zip(&whole) {
            assert_eq!((s.coord, &input[s.start..s.end]), (*c, body.as_slice()));
        }
        let increasing = whole.windows(2).all(|w| w[0].0 < w[1].0);
        if increasing {
            for (c, body) in &whole {
                assert_eq!(find(&input, *c).map(|r| &input[r]), Some(body.as_slice()));
            }
        }

        if let Ok(text) = std::str::from_utf8(&input) {
            utf8_cases += 1;
            let theirs: Vec<(Coordinate, Vec<u8>)> = reference::phokenize(text)
                .into_iter()
                .filter(|p| !p.scroll.is_empty())
                .map(|p| {
                    let r = p.coord;
                    let d = [
                        r.z.library,
                        r.z.shelf,
                        r.z.series,
                        r.y.collection,
                        r.y.volume,
                        r.y.book,
                        r.x.chapter,
                        r.x.section,
                        r.x.scroll,
                    ];
                    (Coordinate(d.map(|v| v as u16)), p.scroll.into_bytes())
                })
                .collect();
            assert_eq!(whole, theirs);
            if increasing {
                let mut enc = Encoder::new();
                let mut out = Vec::new();
                for (c, body) in &whole {
                    enc.write_scroll(&mut out, *c, body).unwrap();
                }
                assert_eq!(out, reference::normalize(text).into_bytes());
            }
        }

        let mut r = BundleReader::new();
        for piece in input.chunks(split[0]) {
            let _ = r.feed(piece, |_| {});
        }
        let _ = r.finish(|_| {});
    }
    assert!(utf8_cases > 5000, "only {utf8_cases} UTF-8 cases");
}

#[test]
fn bundle_reader_survives_mutated_bundles() {
    let vector = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-vectors/bundle-basic.phext"
    ))
    .unwrap();
    let mut rng = Rng(0xF022_0002);
    for _ in 0..20_000 {
        let mut b = vector.clone();
        for _ in 0..1 + rng.below(4) {
            let i = rng.below(b.len() as u64) as usize;
            match rng.below(3) {
                0 => b[i] = rng.next() as u8,
                1 => b[i] = BREAKS[rng.below(9) as usize],
                _ => {
                    b.remove(i);
                }
            }
        }
        let mut r = BundleReader::new();
        for piece in b.chunks(1 + rng.below(16) as usize) {
            let _ = r.feed(piece, |_| {});
        }
        let _ = r.finish(|_| {});
    }
}
