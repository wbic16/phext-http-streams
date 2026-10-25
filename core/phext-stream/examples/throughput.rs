//! Rough decoder/validator throughput, plus the libphext 0.3.1 phokenize baseline on a smaller input
//! (it is quadratic). Run: cargo run --release -p phext-stream --example throughput
use phext_stream::{validate, Decoder, Event};
use std::time::Instant;

fn doc(len: usize, scroll_len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    let line = b"The quick brown fox jumps over the lazy dog. ";
    while v.len() < len {
        for _ in 0..scroll_len / line.len() {
            v.extend_from_slice(line);
        }
        v.push(if v.len().is_multiple_of(7) { 0x18 } else { 0x17 });
    }
    v
}

fn decode_all(buf: &[u8]) -> (usize, usize) {
    let (mut scrolls, mut bytes) = (0, 0);
    let mut d = Decoder::new();
    let mut on = |ev: Event<'_>| match ev {
        Event::ScrollStart(_) => scrolls += 1,
        Event::Data(x) => bytes += x.len(),
        Event::ScrollEnd(_) => {}
    };
    for chunk in buf.chunks(16 * 1024) {
        d.feed(chunk, &mut on);
    }
    d.finish(&mut on);
    (scrolls, bytes)
}

fn main() {
    let big = doc(256 << 20, 2048);
    let t = Instant::now();
    let (scrolls, bytes) = decode_all(&big);
    let s = t.elapsed().as_secs_f64();
    println!(
        "phext-stream decode: {} MiB, {scrolls} scrolls, {bytes} content bytes: {:.0} MiB/s",
        big.len() >> 20,
        (big.len() >> 20) as f64 / s
    );

    let clean: Vec<u8> = big
        .iter()
        .copied()
        .filter(|&b| b != 0x17 && b != 0x18)
        .collect();
    let t = Instant::now();
    assert!(std::hint::black_box(validate(std::hint::black_box(&clean))).is_ok());
    println!(
        "phext-stream validate: {:.0} MiB/s",
        (clean.len() >> 20) as f64 / t.elapsed().as_secs_f64()
    );

    let small = doc(4 << 20, 2048);
    let text = std::str::from_utf8(&small).unwrap();
    let t = Instant::now();
    let n = libphext::phext::phokenize(text).len();
    let s_ref = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let _ = decode_all(&small);
    let s_ours = t.elapsed().as_secs_f64();
    println!(
        "4 MiB doc ({n} scrolls): libphext phokenize {:.3}s, phext-stream {:.4}s ({:.0}x)",
        s_ref,
        s_ours,
        s_ref / s_ours
    );
}
