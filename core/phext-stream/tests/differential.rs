//! phext-stream must agree with libphext 0.3.1 (the reference implementation) on every input.

mod common;

use common::{c, decode_chunked, random_phext, Rng};
use libphext::phext as reference;
use phext_stream::{find, Coordinate, Encoder};

fn from_ref(r: &reference::Coordinate) -> Coordinate {
    Coordinate([
        r.z.library as u16,
        r.z.shelf as u16,
        r.z.series as u16,
        r.y.collection as u16,
        r.y.volume as u16,
        r.y.book as u16,
        r.x.chapter as u16,
        r.x.section as u16,
        r.x.scroll as u16,
    ])
}

fn reference_scrolls(s: &str) -> Vec<(Coordinate, Vec<u8>)> {
    reference::phokenize(s)
        .into_iter()
        .filter(|p| !p.scroll.is_empty())
        .map(|p| (from_ref(&p.coord), p.scroll.into_bytes()))
        .collect()
}

fn encode(scrolls: &[(Coordinate, Vec<u8>)]) -> Vec<u8> {
    let mut enc = Encoder::new();
    let mut out = Vec::new();
    for (at, content) in scrolls {
        enc.write_scroll(&mut out, *at, content).unwrap();
    }
    out
}

const CHUNKINGS: [&[usize]; 4] = [&[usize::MAX], &[1], &[3, 1, 7], &[64, 2, 1000]];

#[test]
fn decoder_matches_phokenize_on_random_documents() {
    let mut rng = Rng(0x5EED_0001);
    for round in 0..3000 {
        let len = 1 + rng.below(400) as usize;
        let doc = random_phext(&mut rng, len);
        let want = reference_scrolls(&doc);
        for chunks in CHUNKINGS {
            let got = decode_chunked(doc.as_bytes(), chunks);
            assert_eq!(got, want, "round {round}, chunks {chunks:?}, doc {doc:?}");
        }
    }
}

#[test]
fn encoder_matches_normalize_on_random_documents() {
    let mut rng = Rng(0x5EED_0002);
    for round in 0..3000 {
        let len = 1 + rng.below(400) as usize;
        let doc = random_phext(&mut rng, len);
        let scrolls = reference_scrolls(&doc);
        let ours = encode(&scrolls);
        let theirs = reference::normalize(&doc);
        assert_eq!(
            String::from_utf8(ours).unwrap(),
            theirs,
            "round {round}, doc {doc:?}"
        );
    }
}

#[test]
fn find_matches_fetch_on_random_documents() {
    let mut rng = Rng(0x5EED_0003);
    for _ in 0..1000 {
        let len = 1 + rng.below(300) as usize;
        let doc = random_phext(&mut rng, len);
        let scrolls = reference_scrolls(&doc);
        for (at, content) in &scrolls {
            let r = find(doc.as_bytes(), *at).expect("listed scroll must be found");
            assert_eq!(&doc.as_bytes()[r], content.as_slice());
            let ref_coord = reference::to_coordinate(&at.to_string());
            assert_eq!(
                reference::fetch(&doc, ref_coord).as_bytes(),
                content.as_slice()
            );
        }
        // a coordinate past the end of the document
        assert_eq!(find(doc.as_bytes(), c("999.1.1/1.1.1/1.1.1")), None);
    }
}

#[test]
fn coordinates_format_like_the_reference() {
    let mut rng = Rng(0x5EED_0004);
    for _ in 0..2000 {
        let mut d = [0u16; 9];
        for v in &mut d {
            *v = 1 + rng.below(999) as u16;
        }
        let ours = Coordinate(d);
        let r = reference::to_coordinate(&ours.to_string());
        assert_eq!(from_ref(&r), ours);
        assert_eq!(r.to_string(), ours.to_string());
        assert_eq!(r.to_urlencoded(), ours.url().to_string());
    }
}

#[test]
fn regression_corpus_from_libphext() {
    // the input that stalled libphext before 0.2.0 (scroll index 100) plus saturation at 999
    let mut doc = String::from("a");
    for _ in 0..120 {
        doc.push('\x17');
    }
    doc.push('b');
    assert_eq!(
        decode_chunked(doc.as_bytes(), &[5]),
        reference_scrolls(&doc)
    );

    let mut doc = String::from("x");
    for _ in 0..1005 {
        doc.push('\x18');
    }
    doc.push('y');
    let got = decode_chunked(doc.as_bytes(), &[1]);
    assert_eq!(got, reference_scrolls(&doc));
    assert_eq!(got[1].0, c("1.1.1/1.1.1/1.999.1"));
}

#[test]
fn shared_vectors_match_the_reference() {
    // core/test-vectors/*.expected.json is generated from phext-stream; this pins the inputs to libphext
    // too, so the Python/C#/Node checkers are effectively compared against the Rust reference.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-vectors");
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "phext") {
            let bytes = std::fs::read(&path).unwrap();
            let text = std::str::from_utf8(&bytes).unwrap();
            assert_eq!(
                decode_chunked(&bytes, &[13]),
                reference_scrolls(text),
                "{}",
                path.display()
            );
            n += 1;
        }
    }
    assert!(n >= 20, "expected the generated vectors, found {n}");
}
