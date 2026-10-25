mod common;

use common::{c, decode_chunked, Rng};
use phext_stream::base64;
use phext_stream::{
    find, index, validate, BundleError, BundleEvent, BundleReader, Coordinate, EncodeError,
    Encoder, Entry, Header, HeaderError, Location,
};

fn entry(loc: Option<&str>, media: &str, b64: bool, path: &str) -> Entry {
    Entry {
        location: loc.map_or(Location::External, |s| Location::Inline(c(s))),
        media_type: media.into(),
        base64: b64,
        path: path.into(),
    }
}

/// Builds a bundle: header + inline scrolls (base64-encoding where flagged).
fn build(entries: &[Entry], bodies: &[&[u8]]) -> Vec<u8> {
    let header = Header {
        entries: entries.to_vec(),
    };
    let mut out = Vec::new();
    let mut enc = Encoder::new();
    enc.write_scroll(&mut out, Coordinate::ORIGIN, header.encode().as_bytes())
        .unwrap();
    let mut inline: Vec<(Coordinate, Vec<u8>)> = entries
        .iter()
        .zip(bodies)
        .filter_map(|(e, b)| match e.location {
            Location::Inline(at) => Some((
                at,
                if e.base64 {
                    base64::encode(b).into_bytes()
                } else {
                    b.to_vec()
                },
            )),
            Location::External => None,
        })
        .collect();
    inline.sort_by_key(|(at, _)| *at);
    for (at, body) in inline {
        enc.write_scroll(&mut out, at, &body).unwrap();
    }
    out
}

#[derive(Debug, Default)]
struct Got {
    header: Option<Header>,
    bodies: Vec<Vec<u8>>,
    order: Vec<String>,
    unlisted: Vec<Coordinate>,
    missing: Vec<usize>,
}

fn read(stream: &[u8], chunk: usize) -> Result<Got, BundleError> {
    let mut got = Got::default();
    let mut r = BundleReader::new();
    let mut on = |ev: BundleEvent<'_>| match ev {
        BundleEvent::Header(h) => {
            got.bodies = vec![Vec::new(); h.entries.len()];
            got.header = Some(h.clone());
            got.order.push("header".into());
        }
        BundleEvent::ResourceStart(i) => got.order.push(format!("start {i}")),
        BundleEvent::ResourceData(i, d) => got.bodies[i].extend_from_slice(d),
        BundleEvent::ResourceEnd(i) => got.order.push(format!("end {i}")),
        BundleEvent::Unlisted(c) => got.unlisted.push(c),
    };
    for piece in stream.chunks(chunk.max(1)) {
        r.feed(piece, &mut on)?;
    }
    got.missing = r.finish(&mut on)?;
    Ok(got)
}

#[test]
fn end_to_end_bundle_in_every_chunk_size() {
    let png: Vec<u8> = (0u8..=255).cycle().take(1000).collect(); // contains every break byte
    assert!(validate(&png).is_err());
    let entries = [
        entry(Some("1.1.1/1.1.1/1.1.2"), "text/html", false, "/index.html"),
        entry(Some("1.1.1/1.1.1/1.1.3"), "text/css", false, "/app.css"),
        entry(Some("1.1.1/1.1.1/1.2.1"), "image/png", true, "/icon.png"),
        entry(None, "image/jpeg", false, "/hero.jpg"),
        entry(
            Some("2.1.1/1.1.1/1.1.1"),
            "application/json",
            false,
            "/channel/2.json",
        ),
    ];
    let bodies: [&[u8]; 5] = [
        b"<!doctype html><p>hi \xE2\x86\x92</p>\n",
        b"p{color:red}",
        &png,
        b"",
        b"{\"ok\":true}",
    ];
    let stream = build(&entries, &bodies);

    for chunk in [1, 2, 3, 5, 7, 64, 4096] {
        let got = read(&stream, chunk).unwrap();
        assert_eq!(
            got.header.as_ref().unwrap().entries,
            entries,
            "chunk {chunk}"
        );
        assert_eq!(got.bodies[0], bodies[0]);
        assert_eq!(got.bodies[1], bodies[1]);
        assert_eq!(got.bodies[2], png, "base64 decoded across chunk {chunk}");
        assert!(got.bodies[3].is_empty());
        assert_eq!(got.bodies[4], bodies[4]);
        assert_eq!(
            got.order,
            [
                "header", "start 0", "end 0", "start 1", "end 1", "start 2", "end 2", "start 4",
                "end 4"
            ]
        );
        assert!(got.missing.is_empty() && got.unlisted.is_empty());
    }
}

#[test]
fn header_arrives_before_any_resource_byte() {
    // the client must be able to start external fetches as soon as the header scroll closes
    let entries = [
        entry(None, "font/woff2", false, "/f.woff2"),
        entry(Some("1.1.1/1.1.1/1.1.2"), "text/plain", false, "/a"),
    ];
    let stream = build(&entries, &[b"", b"body"]);
    let header_end = stream.iter().position(|&b| b == 0x17).unwrap();
    let mut r = BundleReader::new();
    let mut saw_header = false;
    r.feed(&stream[..=header_end], |ev| {
        saw_header |= matches!(ev, BundleEvent::Header(_))
    })
    .unwrap();
    assert!(
        saw_header,
        "header event must fire on the break that closes it"
    );
}

#[test]
fn missing_and_unlisted_scrolls() {
    let entries = [
        entry(Some("1.1.1/1.1.1/1.1.2"), "text/plain", false, "/a"),
        entry(
            Some("1.1.1/1.1.1/1.1.4"),
            "text/plain",
            false,
            "/never-sent",
        ),
    ];
    let header = Header {
        entries: entries.to_vec(),
    };
    let mut s = Vec::new();
    let mut enc = Encoder::new();
    enc.write_scroll(&mut s, Coordinate::ORIGIN, header.encode().as_bytes())
        .unwrap();
    enc.write_scroll(&mut s, c("1.1.1/1.1.1/1.1.2"), b"a")
        .unwrap();
    enc.write_scroll(&mut s, c("1.1.1/1.1.1/1.1.3"), b"stray")
        .unwrap();
    let got = read(&s, 3).unwrap();
    assert_eq!(got.missing, [1]);
    assert_eq!(got.unlisted, [c("1.1.1/1.1.1/1.1.3")]);
    assert_eq!(got.bodies[0], b"a");
}

#[test]
fn reader_errors() {
    // first scroll not at the origin
    assert_eq!(
        read(b"\x17phext-bundle 1\n", 4).unwrap_err(),
        BundleError::MissingHeader(c("1.1.1/1.1.1/1.1.2"))
    );
    // empty stream
    assert!(matches!(
        read(b"", 1).unwrap_err(),
        BundleError::MissingHeader(_)
    ));
    // bad magic
    assert_eq!(
        read(b"phext-bundle 2\n\x17x", 2).unwrap_err(),
        BundleError::Header(HeaderError::Magic)
    );
    // oversized header
    let mut r = BundleReader::with_max_header(8);
    assert_eq!(
        r.feed(b"phext-bundle 1\n", |_| {}).unwrap_err(),
        BundleError::HeaderTooLarge
    );
    assert_eq!(
        r.feed(b"more", |_| {}).unwrap_err(),
        BundleError::HeaderTooLarge,
        "errors are sticky"
    );
    // corrupt base64
    let entries = [entry(
        Some("1.1.1/1.1.1/1.1.2"),
        "image/png",
        true,
        "/x.png",
    )];
    let header = Header {
        entries: entries.to_vec(),
    };
    let mut s = Vec::new();
    let mut enc = Encoder::new();
    enc.write_scroll(&mut s, Coordinate::ORIGIN, header.encode().as_bytes())
        .unwrap();
    enc.write_scroll(&mut s, c("1.1.1/1.1.1/1.1.2"), b"AAA!")
        .unwrap();
    assert!(matches!(
        read(&s, 1).unwrap_err(),
        BundleError::Base64 { index: 0, .. }
    ));
    // truncated base64
    let mut s2 = s[..s.len() - 4].to_vec();
    s2.extend_from_slice(b"AAA");
    assert!(matches!(
        read(&s2, 2).unwrap_err(),
        BundleError::Base64 { index: 0, .. }
    ));
}

#[test]
fn header_parse_rules() {
    let ok =
        "phext-bundle 1\r\n# comment\n\n1.1.1;1.1.1;1.1.2 text/html /\nexternal image/png /p.png\n";
    let h = Header::parse(ok.as_bytes()).unwrap();
    assert_eq!(h.entries.len(), 2);
    assert_eq!(
        h.entries[0].location,
        Location::Inline(c("1.1.1/1.1.1/1.1.2"))
    );
    assert_eq!(
        Header::parse(h.encode().as_bytes()).unwrap(),
        h,
        "encode/parse round trip"
    );

    let bad: [(&str, &str); 8] = [
        (
            "phext-bundle 1\n1.1.1/1.1.1/1.1.1 text/html /a",
            "origin reserved",
        ),
        (
            "phext-bundle 1\n1.1.1/1.1.1/1.1.2 text/html /a\n1.1.1/1.1.1/1.1.2 text/css /b",
            "dup coord",
        ),
        (
            "phext-bundle 1\n1.1.1/1.1.1/1.1.2 text/html /a\n1.1.1/1.1.1/1.1.3 text/css /a",
            "dup path",
        ),
        (
            "phext-bundle 1\nexternal image/png;base64 /a",
            "base64 external",
        ),
        (
            "phext-bundle 1\n1.1.1/1.1.1/1.1.2 text/html a",
            "relative path",
        ),
        (
            "phext-bundle 1\n1.1.1/1.1.1/1.1.2 text/html",
            "too few fields",
        ),
        (
            "phext-bundle 1\n1.1.1/1.1.1/1.1.2 text/html /a extra",
            "too many fields",
        ),
        (
            "phext-bundle 1\n1.1.1/1.1.1 text/html /a",
            "short coordinate",
        ),
    ];
    for (input, why) in bad {
        assert!(
            Header::parse(input.as_bytes()).is_err(),
            "{why} should be rejected"
        );
    }
    assert_eq!(Header::parse(b"\xFF").unwrap_err(), HeaderError::NotUtf8);
}

#[test]
fn coordinate_parse_rules() {
    assert_eq!(c("1.2.3/4.5.6/7.8.9").0, [1, 2, 3, 4, 5, 6, 7, 8, 9]);
    assert_eq!(c("1.2.3;4.5.6;7.8.9"), c("1.2.3/4.5.6/7.8.9"));
    assert_eq!(
        c("999.999.999/999.999.999/999.999.999").to_string().len(),
        35
    );
    for bad in [
        "",
        "1.1.1/1.1.1/1.1",
        "1.1.1/1.1.1/1.1.1.1",
        "1/1.1.1.1/1.1.1",
        "1.1.1.1.1.1.1.1.1",
        "0.1.1/1.1.1/1.1.1",
        "1000.1.1/1.1.1/1.1.1",
        "1.1.1/1.1.1/1.1.x",
        "1..1/1.1.1/1.1.1",
        "1.1.1/1.1.1/1.1.99999999999",
    ] {
        assert!(
            Coordinate::parse(bad).is_err(),
            "{bad:?} should be rejected"
        );
    }
}

#[test]
fn encoder_rules() {
    let mut e = Encoder::new();
    assert_eq!(e.seek(Coordinate::ORIGIN).unwrap().count(), 0);
    assert!(matches!(
        e.seek(Coordinate::ORIGIN),
        Err(EncodeError::OutOfOrder { .. })
    ));
    assert_eq!(
        e.seek(c("1.1.1/1.1.1/1.3.2")).unwrap().collect::<Vec<_>>(),
        [0x18, 0x18, 0x17]
    );
    assert_eq!(
        e.seek(c("1.1.2/1.1.1/1.1.1")).unwrap().collect::<Vec<_>>(),
        [0x1E]
    );
    assert!(matches!(
        e.seek(Coordinate([1, 1, 2, 1, 1, 1, 1, 1, 1000])),
        Err(EncodeError::Invalid(_))
    ));
    let mut out = Vec::new();
    assert!(
        matches!(e.write_scroll(&mut out, c("1.1.3/1.1.1/1.1.1"), b"a\x1Fb"), Err(EncodeError::Forbidden(f)) if f.offset == 1)
    );
    assert!(out.is_empty(), "nothing written on error");
}

#[test]
fn index_and_find_agree() {
    let mut rng = Rng(42);
    for _ in 0..500 {
        let len = 1 + rng.below(200) as usize;
        let doc = common::random_phext(&mut rng, len);
        let spans = index(doc.as_bytes());
        let scrolls = decode_chunked(doc.as_bytes(), &[7]);
        assert_eq!(spans.len(), scrolls.len());
        for (s, (at, body)) in spans.iter().zip(&scrolls) {
            assert_eq!(s.coord, *at);
            assert_eq!(&doc.as_bytes()[s.start..s.end], body.as_slice());
            assert_eq!(find(doc.as_bytes(), *at), Some(s.start..s.end));
        }
    }
}

#[test]
fn base64_round_trip_and_streaming() {
    let mut rng = Rng(7);
    for n in 0..200 {
        let data: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        let enc = base64::encode(&data);
        assert_eq!(enc.len(), base64::encoded_len(n));
        assert!(
            validate(enc.as_bytes()).is_ok(),
            "base64 never contains break bytes"
        );
        assert_eq!(base64::decode(enc.as_bytes()).unwrap(), data);
        // stream it in uneven pieces
        let mut d = base64::Decoder::new();
        let mut out = Vec::new();
        for piece in enc.as_bytes().chunks(1 + (n % 5)) {
            d.feed(piece, &mut out).unwrap();
        }
        d.finish().unwrap();
        assert_eq!(out, data);
    }
    assert_eq!(base64::encode(b"foobar"), "Zm9vYmFy");
    assert_eq!(base64::encode(b"fo"), "Zm8=");
    for bad in ["Zm8", "Zm8=Zm8=", "Z===", "Zm=v", "Zm 9"] {
        assert!(
            base64::decode(bad.as_bytes()).is_err(),
            "{bad:?} should be rejected"
        );
    }
}
