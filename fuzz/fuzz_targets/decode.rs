//! Properties checked on arbitrary bytes:
//! 1. chunking never changes what the decoder reports;
//! 2. `index` and `find` agree with the decoder;
//! 3. re-encoding the decoded scrolls reproduces libphext's `normalize` (for UTF-8 input);
//! 4. the decoder agrees with libphext `phokenize` (for UTF-8 input);
//! 5. `BundleReader` never panics, whatever it is fed.
#![no_main]

use libfuzzer_sys::fuzz_target;
use phext_stream::{find, index, BundleReader, Coordinate, Decoder, Encoder, Event};

fn decode(input: &[u8], split: &[u8]) -> Vec<(Coordinate, Vec<u8>)> {
    let mut out: Vec<(Coordinate, Vec<u8>)> = Vec::new();
    let mut dec = Decoder::new();
    let mut on = |ev: Event<'_>| match ev {
        Event::ScrollStart(c) => out.push((c, Vec::new())),
        Event::Data(d) => out.last_mut().unwrap().1.extend_from_slice(d),
        Event::ScrollEnd(_) => {}
    };
    let (mut pos, mut k) = (0, 0);
    while pos < input.len() {
        let n = (split.get(k % split.len().max(1)).copied().unwrap_or(255) as usize + 1).min(input.len() - pos);
        dec.feed(&input[pos..pos + n], &mut on);
        pos += n;
        k += 1;
    }
    dec.finish(&mut on);
    out
}

fuzz_target!(|data: &[u8]| {
    let (split, input) = data.split_at(data.len().min(8));
    let whole = decode(input, &[u8::MAX]);
    assert_eq!(decode(input, split), whole);

    let spans = index(input);
    assert_eq!(spans.len(), whole.len());
    for (s, (c, body)) in spans.iter().zip(&whole) {
        assert_eq!((s.coord, &input[s.start..s.end]), (*c, body.as_slice()));
    }
    let increasing = whole.windows(2).all(|w| w[0].0 < w[1].0);
    if increasing {
        for (c, body) in &whole {
            assert_eq!(find(input, *c).map(|r| &input[r]), Some(body.as_slice()));
        }
    }

    if let Ok(text) = std::str::from_utf8(input) {
        let reference: Vec<(Coordinate, Vec<u8>)> = libphext::phext::phokenize(text)
            .into_iter()
            .filter(|p| !p.scroll.is_empty())
            .map(|p| {
                let r = p.coord;
                let d = [r.z.library, r.z.shelf, r.z.series, r.y.collection, r.y.volume, r.y.book, r.x.chapter, r.x.section, r.x.scroll];
                (Coordinate(d.map(|v| v as u16)), p.scroll.into_bytes())
            })
            .collect();
        assert_eq!(whole, reference);
        if increasing {
            let mut enc = Encoder::new();
            let mut out = Vec::new();
            for (c, body) in &whole {
                enc.write_scroll(&mut out, *c, body).unwrap();
            }
            assert_eq!(out, libphext::phext::normalize(text).into_bytes());
        }
    }

    let mut r = BundleReader::new();
    let _ = r.feed(input, |_| {});
    let _ = r.finish(|_| {});
});
