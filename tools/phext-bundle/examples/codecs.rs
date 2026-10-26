//! Size and speed of every encoder on real bundles. Run after `node bench/fixtures.mjs`:
//!   cargo run --release -p phext-bundle --example codecs > bench/results/codecs.md
//!
//! Size decides how many slow-start rounds the bundle needs on the wire; compression speed decides whether
//! a server can build bundles per request (Phase 2 modules) or must pre-build them; decompression speed is
//! what the client pays.

use phext_bundle::codecs::Codec;
use phext_bundle::{slow_start_bytes, INITCWND};
use std::time::Instant;

const CODECS: &[Codec] = &[
    Codec::Flate2(1),
    Codec::Flate2(6),
    Codec::Flate2(9),
    Codec::Libdeflate(1),
    Codec::Libdeflate(6),
    Codec::Libdeflate(9),
    Codec::Libdeflate(12),
    Codec::Zopfli(15),
    Codec::Brotli(1),
    Codec::Brotli(5),
    Codec::Brotli(9),
    Codec::Brotli(11),
    Codec::Zstd(1),
    Codec::Zstd(3),
    Codec::Zstd(9),
    Codec::Zstd(19),
    Codec::Zstd(22),
];

/// Median seconds per call, repeating until ~0.3 s or 25 calls (at least 3).
fn time<F: FnMut()>(mut f: F) -> f64 {
    let mut samples = Vec::new();
    let start = Instant::now();
    while samples.len() < 3 || (samples.len() < 25 && start.elapsed().as_secs_f64() < 0.3) {
        let t = Instant::now();
        f();
        samples.push(t.elapsed().as_secs_f64());
    }
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

/// Slow-start rounds needed to deliver `n` bytes on a fresh connection.
fn rounds(n: usize) -> u32 {
    (1..)
        .find(|&r| slow_start_bytes(r, INITCWND) >= n)
        .unwrap_or(0)
}

fn main() -> std::io::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/out");
    let inputs = [
        ("small", "phext"),
        ("channels", "phext"),
        ("icons", "phext"),
        ("spa", "phext"),
        ("spa", "phext-all"),
    ];
    println!("# Bundle encoders: size and speed\n");
    println!(
        "Each fixture's uncompressed bundle (`app.phext`) through every encoder. MB/s is of uncompressed bytes, \
         single-threaded, median of repeated runs on this machine. \"Rounds\" is how many slow-start round trips \
         (initcwnd 10) the encoded bundle needs; the default budget is 2.\n"
    );
    for (fixture, variant) in inputs {
        let path = root.join(fixture).join(variant).join("app.phext");
        let Ok(data) = std::fs::read(&path) else {
            eprintln!(
                "skipping {}: run node bench/fixtures.mjs first",
                path.display()
            );
            continue;
        };
        let baseline = Codec::Flate2(9).compress(&data)?.len();
        println!(
            "## {fixture} ({variant}): {} bytes uncompressed\n",
            data.len()
        );
        println!("| encoder | bytes | vs gzip -9 | rounds | compress MB/s | decompress MB/s |");
        println!("|---|---|---|---|---|---|");
        for &codec in CODECS {
            let out = codec.compress(&data)?;
            assert_eq!(
                codec.decompress(&out, data.len())?,
                data,
                "{} round trip",
                codec.name()
            );
            let mb = data.len() as f64 / 1e6;
            let c = time(|| {
                std::hint::black_box(codec.compress(std::hint::black_box(&data)).unwrap());
            });
            let d = time(|| {
                std::hint::black_box(
                    codec
                        .decompress(std::hint::black_box(&out), data.len())
                        .unwrap(),
                );
            });
            println!(
                "| {} | {} | {:+.1}% | {} | {:.1} | {:.0} |",
                codec.name(),
                out.len(),
                (out.len() as f64 / baseline as f64 - 1.0) * 100.0,
                rounds(out.len()),
                mb / c,
                mb / d
            );
        }
        println!();
    }
    Ok(())
}
