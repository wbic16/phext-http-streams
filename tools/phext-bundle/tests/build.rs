use flate2::write::GzDecoder;
use phext_bundle::{build, Asset, Decision, Flush, Options};
use phext_stream::{BundleEvent, BundleReader, Location};
use std::io::Write;

fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s as u8
        })
        .collect()
}

fn text(n: usize) -> Vec<u8> {
    let words = [
        "const ",
        "value",
        " = ",
        "render(",
        "props",
        ");\n",
        "return ",
        "function ",
        "{ ",
        "} ",
        "x",
        "42",
    ];
    let mut out = Vec::new();
    let mut i = 7u64;
    while out.len() < n {
        i = i
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        out.extend_from_slice(words[(i >> 33) as usize % words.len()].as_bytes());
    }
    out.truncate(n);
    out
}

fn gunzip(gz: &[u8]) -> Vec<u8> {
    let mut d = GzDecoder::new(Vec::new());
    d.write_all(gz).unwrap();
    d.finish().unwrap()
}

fn read_back(gz: &[u8]) -> (Vec<String>, Vec<Vec<u8>>, Vec<usize>) {
    let mut r = BundleReader::new();
    let mut paths = Vec::new();
    let mut bodies: Vec<Vec<u8>> = Vec::new();
    let mut on = |ev: BundleEvent<'_>| match ev {
        BundleEvent::Header(h) => {
            paths = h.entries.iter().map(|e| e.path.clone()).collect();
            bodies = vec![Vec::new(); h.entries.len()];
        }
        BundleEvent::ResourceData(i, d) => bodies[i].extend_from_slice(d),
        _ => {}
    };
    r.feed(&gunzip(gz), &mut on).unwrap();
    let missing = r.finish(&mut on).unwrap();
    (paths, bodies, missing)
}

#[test]
fn inline_rule() {
    let mut breaky = text(500);
    breaky[100] = 0x1F; // text that can't travel unescaped
    let assets = vec![
        Asset::new("/index.html", text(3000)),
        Asset::new("/app.css", text(2000)),
        Asset::new("/icon.png", noise(1500, 1)), // small binary, fits
        Asset::new("/hero.bmp", noise(60_000, 2)), // over inline_max
        Asset::new("/weird.js", breaky.clone()), // text with a break byte -> base64
        Asset::new("/big-icon.png", noise(5000, 3)), // under inline_max, but over the budget by now
        Asset::new("/empty.txt", Vec::new()),
    ];
    let opts = Options {
        inline_max: 8192,
        flight_budget: 6000,
        ..Options::default()
    };
    let b = build(&assets, &opts).unwrap();
    assert_eq!(
        b.decisions,
        [
            Decision::Text,
            Decision::Text,
            Decision::Base64,
            Decision::External("over inline_max"),
            Decision::Base64,
            Decision::External("over flight_budget"),
            Decision::External("empty"),
        ]
    );
    assert!(b.gz.len() <= 6000, "gz {} over budget", b.gz.len());
    assert_eq!(
        b.links,
        [
            "</hero.bmp>; rel=preload; as=image",
            "</big-icon.png>; rel=preload; as=image",
            "</empty.txt>; rel=preload; as=fetch; crossorigin"
        ]
    );
    assert_eq!(gunzip(&b.gz), b.phext);

    let (paths, bodies, missing) = read_back(&b.gz);
    assert!(missing.is_empty());
    assert_eq!(
        paths,
        assets.iter().map(|a| a.path.clone()).collect::<Vec<_>>()
    );
    for (i, a) in assets.iter().enumerate() {
        if matches!(b.header.entries[i].location, Location::Inline(_)) {
            assert_eq!(bodies[i], a.bytes, "{}", a.path);
        }
    }
}

#[test]
fn text_is_inlined_even_over_budget() {
    let assets = vec![
        Asset::new("/index.html", text(200_000)),
        Asset::new("/a.png", noise(100, 9)),
    ];
    let opts = Options {
        flight_budget: 14_600,
        ..Options::default()
    };
    let b = build(&assets, &opts).unwrap();
    assert!(
        b.gz.len() > opts.flight_budget,
        "the text alone must exceed the budget"
    );
    assert_eq!(
        b.decisions,
        [Decision::Text, Decision::External("over flight_budget")]
    );
}

#[test]
fn many_resources_get_valid_increasing_coordinates() {
    let assets: Vec<Asset> = (0..2500)
        .map(|i| {
            Asset::new(
                format!("/c/{i}.json"),
                format!("{{\"i\":{i}}}").into_bytes(),
            )
        })
        .collect();
    let b = build(&assets, &Options::default()).unwrap();
    let (paths, bodies, missing) = read_back(&b.gz);
    assert!(missing.is_empty());
    assert_eq!(paths.len(), 2500);
    assert_eq!(bodies[2499], b"{\"i\":2499}");
}

/// Decompresses one compressed byte at a time and records every decompressed length that becomes
/// available; a sync flush makes the plain stream available exactly up to the flush point.
fn reachable_lengths(gz: &[u8]) -> Vec<usize> {
    let mut d = GzDecoder::new(Vec::new());
    let mut seen = Vec::new();
    for &b in gz {
        d.write_all(&[b]).unwrap();
        d.flush().unwrap();
        let n = d.get_ref().len();
        if seen.last() != Some(&n) {
            seen.push(n);
        }
    }
    seen
}

#[test]
fn flush_points_close_each_scroll() {
    let assets: Vec<Asset> = (0..6)
        .map(|i| Asset::new(format!("/f{i}.js"), text(4000 + i * 100)))
        .collect();
    let all = build(
        &assets,
        &Options {
            flush: Flush::All,
            ..Options::default()
        },
    )
    .unwrap();
    let none = build(
        &assets,
        &Options {
            flush: Flush::None,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(all.phext, none.phext);
    assert!(all.gz.len() > none.gz.len());

    // every scroll closes at its next break byte: the byte after each content run
    let closes: Vec<usize> = all
        .phext
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, &b)| b == 0x17)
        .map(|(i, _)| i + 1)
        .collect();
    assert_eq!(closes.len(), 6);
    let reach = reachable_lengths(&all.gz);
    for c in &closes {
        assert!(
            reach.contains(c),
            "scroll close at {c} never became available on its own"
        );
    }
    let reach_none = reachable_lengths(&none.gz);
    assert!(closes.iter().filter(|c| reach_none.contains(c)).count() < closes.len());
}

#[test]
fn defaults_are_inline_everything_and_one_shot_gzip() {
    use phext_bundle::{slow_start_bytes, INITCWND, UNLIMITED};
    assert_eq!(slow_start_bytes(1, INITCWND), 14_600);
    assert_eq!(slow_start_bytes(2, INITCWND), 43_800);
    assert_eq!(slow_start_bytes(3, INITCWND), 102_200);
    let d = Options::default();
    assert_eq!((d.inline_max, d.flight_budget), (UNLIMITED, UNLIMITED));
    assert_eq!(d.flush, Flush::None);

    // large binaries and break-carrying text all ride inline; nothing is left external
    let mut breaky = text(3000);
    breaky[10] = 0x01;
    let assets = vec![
        Asset::new("/index.html", text(5000)),
        Asset::new("/hero.bmp", noise(300_000, 4)),
        Asset::new("/odd.js", breaky),
    ];
    let b = build(&assets, &d).unwrap();
    assert_eq!(
        b.decisions,
        [Decision::Text, Decision::Base64, Decision::Base64]
    );
    assert!(b.links.is_empty());
    let (_, bodies, missing) = read_back(&b.gz);
    assert!(missing.is_empty());
    for (got, a) in bodies.iter().zip(&assets) {
        assert_eq!(got, &a.bytes, "{}", a.path);
    }

    // one-shot output decompresses to the same stream and is never larger than a flushed one
    let assets: Vec<Asset> = (0..6)
        .map(|i| Asset::new(format!("/f{i}.js"), text(4000 + i * 100)))
        .collect();
    let one = build(&assets, &d).unwrap();
    let flushed = build(
        &assets,
        &Options {
            flush: Flush::All,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(gunzip(&one.gz), one.phext);
    assert!(one.gz.len() < flushed.gz.len());
}
