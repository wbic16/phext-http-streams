//! phext-bundle: directory in, `NAME.phext` + `NAME.phext.gz` + `NAME.links` + `NAME.report.json` out.

use phext_bundle::codecs::Codec;
use phext_bundle::{build, collect, Decision, Flush, Options};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: phext-bundle <dir> -o <out-prefix> [--entry index.html] [--order FILE]
                    [--inline-max BYTES] [--flight-budget BYTES | --budget-rounds N]
                    [--flush none|header|all] [--gzip CODEC] [--also CODEC,...] [--quiet]

Writes <out-prefix>.phext, .phext.gz, .links (Link: preload values for external entries),
and .report.json. --order lists paths in priority order, one per line; others follow by type.

By default everything is inlined: text as-is, binaries as base64. Budgeted mode leaves some binaries
external (fetched over plain HTTP/2): --inline-max skips binaries whose base64 is larger than BYTES,
and --flight-budget / --budget-rounds cap the gzip'd bundle (--budget-rounds 2 = 43800 bytes, what
slow start delivers in HTTP/2's request -> response -> request window). gzip is one-shot unless
--flush asks for sync-flush points.

--gzip picks the gzip encoder for .phext.gz: libdeflate[:1-12] (default libdeflate:12), flate2[:0-9]
(default with --flush; the only one that can flush), or zopfli[:iterations]. --also writes extra
Content-Encodings: br[:0-11] -> .phext.br, zstd[:1-22] -> .phext.zst. The budget is always
measured with flate2:9; the defaults here (libdeflate:12, br:11, zstd:19) all come out smaller,
but low levels can be larger.";

struct Args {
    dir: PathBuf,
    out: PathBuf,
    entry: String,
    order: Vec<String>,
    opts: Options,
    gzip: Codec,
    also: Vec<Codec>,
    quiet: bool,
}

fn parse() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let (mut gzip, mut also) = (None, Vec::new());
    let (mut dir, mut out, mut entry, mut order, mut opts, mut quiet) = (
        None,
        None,
        "index.html".to_string(),
        Vec::new(),
        Options::default(),
        false,
    );
    while let Some(a) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "-o" | "--out" => out = Some(PathBuf::from(val()?)),
            "--entry" => entry = val()?,
            "--order" => {
                let f = val()?;
                let text = std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?;
                order = text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(String::from)
                    .collect();
            }
            "--inline-max" => {
                opts.inline_max = val()?.parse().map_err(|e| format!("--inline-max: {e}"))?
            }
            "--flight-budget" => {
                opts.flight_budget = val()?
                    .parse()
                    .map_err(|e| format!("--flight-budget: {e}"))?
            }
            "--budget-rounds" => {
                let n: u32 = val()?
                    .parse()
                    .map_err(|e| format!("--budget-rounds: {e}"))?;
                if !(1..=20).contains(&n) {
                    return Err("--budget-rounds: expected 1..=20".into());
                }
                opts.flight_budget = phext_bundle::slow_start_bytes(n, phext_bundle::INITCWND);
            }
            "--flush" => {
                opts.flush = match val()?.as_str() {
                    "all" => Flush::All,
                    "header" => Flush::Header,
                    "none" => Flush::None,
                    other => return Err(format!("--flush: unknown mode {other}")),
                }
            }
            "--gzip" => {
                let codec = Codec::parse(&val()?)?;
                gzip = Some(codec);
                if codec.content_encoding().0 != "gzip" {
                    return Err(
                        "--gzip: use flate2, libdeflate, or zopfli (br and zstd go in --also)"
                            .into(),
                    );
                }
            }
            "--also" => {
                for c in val()?.split(',').filter(|c| !c.is_empty()) {
                    let codec = Codec::parse(c)?;
                    if codec.content_encoding().0 == "gzip" {
                        return Err("--also: gzip encoders go in --gzip".into());
                    }
                    also.push(codec);
                }
            }
            "--quiet" | "-q" => quiet = true,
            "-h" | "--help" => return Err(String::new()),
            _ if dir.is_none() && !a.starts_with('-') => dir = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument {a}")),
        }
    }
    // Default gzip encoder: libdeflate -12 for one-shot bundles (~10% smaller than zlib -9 at a fraction of
    // zopfli's cost; see bench/results/codecs.md), flate2 when flush points are wanted.
    let gzip = match (gzip, opts.flush) {
        (None, Flush::None) => Codec::Libdeflate(12),
        (None, _) => Codec::Flate2(9),
        (Some(Codec::Flate2(l)), _) => Codec::Flate2(l),
        (Some(_), f) if f != Flush::None => return Err("--flush needs the flate2 encoder".into()),
        (Some(c), _) => c,
    };
    Ok(Args {
        gzip,
        also,
        dir: dir.ok_or("missing <dir>")?,
        out: out.ok_or("missing -o <out-prefix>")?,
        entry,
        order,
        opts,
        quiet,
    })
}

fn json_str(s: &str) -> String {
    let mut o = String::from('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn run(args: Args) -> std::io::Result<()> {
    let assets = collect(&args.dir, &args.entry, &args.order)?;
    let built = build(&assets, &args.opts)?;
    let with_ext = |ext: &str| {
        let mut p = args.out.clone().into_os_string();
        p.push(ext);
        PathBuf::from(p)
    };
    if let Some(parent) = args.out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(with_ext(".phext"), &built.phext)?;
    // the flate2:9 stream was built alongside the budget check (with any flush points); other encoders
    // recompress the same plain stream
    let gz = match args.gzip {
        Codec::Flate2(9) => built.gz.clone(),
        other => other.compress(&built.phext)?,
    };
    std::fs::write(with_ext(".phext.gz"), &gz)?;
    let mut encodings = vec![format!("\"gzip\": {}", gz.len())];
    for codec in &args.also {
        let (token, ext) = codec.content_encoding();
        let bytes = codec.compress(&built.phext)?;
        encodings.push(format!("\"{token}\": {}", bytes.len()));
        std::fs::write(with_ext(&format!(".phext.{ext}")), &bytes)?;
    }
    std::fs::write(
        with_ext(".links"),
        built
            .links
            .iter()
            .map(|l| format!("{l}\n"))
            .collect::<String>(),
    )?;

    let mut rows = Vec::new();
    for (a, d) in assets.iter().zip(&built.decisions) {
        let (kind, why) = match d {
            Decision::Text => ("inline", ""),
            Decision::Base64 => ("base64", ""),
            Decision::External(why) => ("external", *why),
        };
        rows.push(format!(
            "    {{\"path\": {}, \"type\": {}, \"bytes\": {}, \"decision\": \"{kind}\", \"reason\": \"{why}\"}}",
            json_str(&a.path),
            json_str(a.media_type),
            a.bytes.len()
        ));
        if !args.quiet {
            let why = if why.is_empty() {
                String::new()
            } else {
                format!(" ({why})")
            };
            println!("{:>9}  {:>8}  {}{}", kind, a.bytes.len(), a.path, why);
        }
    }
    let report = format!(
        "{{\n  \"phext_bytes\": {},\n  \"gzip_bytes\": {},\n  \"gzip_encoder\": {},\n  \"encodings\": {{{}}},\n  \"flight_budget\": {},\n  \"inline_max\": {},\n  \"external\": [{}],\n  \"assets\": [\n{}\n  ]\n}}\n",
        built.phext.len(),
        gz.len(),
        json_str(&args.gzip.name()),
        encodings.join(", "),
        args.opts.flight_budget,
        args.opts.inline_max,
        built.links.iter().map(|l| json_str(l)).collect::<Vec<_>>().join(", "),
        rows.join(",\n")
    );
    std::fs::write(with_ext(".report.json"), report)?;
    if !args.quiet {
        let ext = built
            .decisions
            .iter()
            .filter(|d| matches!(d, Decision::External(_)))
            .count();
        println!(
            "bundle: {} bytes, {} gzip via {} ({} budget), {} inline, {} external",
            built.phext.len(),
            gz.len(),
            args.gzip.name(),
            args.opts.flight_budget,
            assets.len() - ext,
            ext
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    match parse() {
        Err(e) => {
            if !e.is_empty() {
                eprintln!("phext-bundle: {e}");
            }
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Ok(args) => match run(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("phext-bundle: {e}");
                ExitCode::FAILURE
            }
        },
    }
}
