//! Content-Encodings a bundle can be pre-compressed with. gzip is the baseline every client supports;
//! zopfli and libdeflate produce standard gzip (smaller, or faster, than zlib-style flate2); brotli and zstd
//! are separate Content-Encodings that Chromium accepts over HTTPS.

use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{self, Read, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// flate2 (miniz_oxide backend), level 0..=9. The only one that supports sync-flush points.
    Flate2(u32),
    /// libdeflate, level 1..=12: whole-buffer only, but fast and tighter than zlib at the same level.
    Libdeflate(i32),
    /// zopfli with this many iterations: the smallest gzip, very slow; for offline builds.
    Zopfli(u64),
    /// brotli quality 0..=11 (window 22).
    Brotli(u32),
    /// zstd level 1..=22.
    Zstd(i32),
}

impl Codec {
    /// The HTTP Content-Encoding token and the file extension the servers look for.
    pub fn content_encoding(self) -> (&'static str, &'static str) {
        match self {
            Codec::Flate2(_) | Codec::Libdeflate(_) | Codec::Zopfli(_) => ("gzip", "gz"),
            Codec::Brotli(_) => ("br", "br"),
            Codec::Zstd(_) => ("zstd", "zst"),
        }
    }

    pub fn name(self) -> String {
        match self {
            Codec::Flate2(l) => format!("gzip flate2 -{l}"),
            Codec::Libdeflate(l) => format!("gzip libdeflate -{l}"),
            Codec::Zopfli(i) => format!("gzip zopfli i{i}"),
            Codec::Brotli(q) => format!("br q{q}"),
            Codec::Zstd(l) => format!("zstd -{l}"),
        }
    }

    /// Parses `flate2:9`, `libdeflate:12`, `zopfli:15`, `br:11`, `zstd:19` (level optional).
    pub fn parse(s: &str) -> Result<Codec, String> {
        let (name, level) = s.split_once(':').map_or((s, None), |(n, l)| (n, Some(l)));
        let lvl =
            |d: i64| level.map_or(Ok(d), |l| l.parse::<i64>().map_err(|e| format!("{s}: {e}")));
        Ok(match name {
            "flate2" | "gzip" => Codec::Flate2(lvl(9)?.clamp(0, 9) as u32),
            "libdeflate" => Codec::Libdeflate(lvl(12)?.clamp(1, 12) as i32),
            "zopfli" => Codec::Zopfli(lvl(15)?.max(1) as u64),
            "br" | "brotli" => Codec::Brotli(lvl(11)?.clamp(0, 11) as u32),
            "zstd" => Codec::Zstd(lvl(19)?.clamp(1, 22) as i32),
            _ => {
                return Err(format!(
                    "unknown codec {name}; expected flate2, libdeflate, zopfli, br, or zstd"
                ))
            }
        })
    }

    pub fn compress(self, data: &[u8]) -> io::Result<Vec<u8>> {
        match self {
            Codec::Flate2(level) => {
                let mut e = GzEncoder::new(Vec::new(), Compression::new(level));
                e.write_all(data)?;
                e.finish()
            }
            Codec::Libdeflate(level) => {
                let lvl = libdeflater::CompressionLvl::new(level)
                    .map_err(|e| io::Error::other(format!("{e:?}")))?;
                let mut c = libdeflater::Compressor::new(lvl);
                let mut out = vec![0; c.gzip_compress_bound(data.len())];
                let n = c
                    .gzip_compress(data, &mut out)
                    .map_err(|e| io::Error::other(format!("{e:?}")))?;
                out.truncate(n);
                Ok(out)
            }
            Codec::Zopfli(iterations) => {
                let opts = zopfli::Options {
                    iteration_count: std::num::NonZeroU64::new(iterations.max(1))
                        .unwrap_or(std::num::NonZeroU64::MIN),
                    ..Default::default()
                };
                let mut out = Vec::new();
                zopfli::compress(opts, zopfli::Format::Gzip, data, &mut out)?;
                Ok(out)
            }
            Codec::Brotli(q) => {
                let mut out = Vec::new();
                let params = brotli::enc::BrotliEncoderParams {
                    quality: q as i32,
                    lgwin: 22,
                    ..Default::default()
                };
                brotli::BrotliCompress(&mut &data[..], &mut out, &params)?;
                Ok(out)
            }
            Codec::Zstd(level) => zstd::bulk::compress(data, level),
        }
    }

    /// Decompresses with the matching decoder (for round-trip checks and the speed benchmark).
    pub fn decompress(self, data: &[u8], expected_len: usize) -> io::Result<Vec<u8>> {
        match self {
            Codec::Flate2(_) | Codec::Zopfli(_) => {
                let mut out = Vec::with_capacity(expected_len);
                flate2::read::GzDecoder::new(data).read_to_end(&mut out)?;
                Ok(out)
            }
            Codec::Libdeflate(_) => {
                let mut d = libdeflater::Decompressor::new();
                let mut out = vec![0; expected_len];
                let n = d
                    .gzip_decompress(data, &mut out)
                    .map_err(|e| io::Error::other(format!("{e:?}")))?;
                out.truncate(n);
                Ok(out)
            }
            Codec::Brotli(_) => {
                let mut out = Vec::with_capacity(expected_len);
                brotli::BrotliDecompress(&mut &data[..], &mut out)?;
                Ok(out)
            }
            Codec::Zstd(_) => zstd::bulk::decompress(data, expected_len),
        }
    }
}
