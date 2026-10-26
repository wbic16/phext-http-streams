# Server modules

Serve phext bundles built on the fly from a directory, from **nginx** (`ngx_http_phext_module`) and **Apache
httpd** (`mod_phext`). Both are thin C layers over one Rust static library,
[`phext-server`](phext-server/include/phext_server.h). It:

- walks the directory and builds the bundle (inline everything, spec §3.2; optional priority order file);
- pre-encodes it once per Content-Encoding (gzip via libdeflate, brotli, zstd), so each request is a
  memory copy at most;
- fingerprints the directory (paths, sizes, mtimes; a stat-only walk), so the server rebuilds only
  when files change. The walk runs off the request path (nginx: a per-worker timer; httpd: a watcher thread),
  since on slow filesystems it is far from free: one walk of a 100-file directory took 607 ms on a WSL `/mnt/c`
  (9P) share, versus 0.15 ms on ext4.

Per request the module picks the **smallest encoding the client accepts** (honoring `q=0`), and sets
`Vary: Accept-Encoding`, a strong per-encoding `ETag` (conditional requests return 304), and `Cache-Control:
no-cache`. Bodies go out without copying; each request holds a reference, so a rebuild never frees a bundle
that is still being sent. Precompressed siblings (`*.gz`, `*.br`, `*.zst`) and hidden files are never
bundled. Pre-built bundles (`phext-bundle` + `gzip_static`, [nginx/phext-static.conf](nginx/phext-static.conf))
remain the zero-CPU option.

Both modules also serve single scrolls by coordinate: `GET /doc.phext;1.1.1;1.1.1;1.1.3` returns that
scroll of `doc.phext` (400 for a malformed coordinate, 404 if empty). Plain `/doc.phext` is still served
whole by the static handler.

## nginx

```nginx
load_module modules/ngx_http_phext_module.so;

location = /app.phext {
    phext_bundle        /srv/site;          # directory to bundle
    phext_bundle_entry  index.html;         # entry document (default)
    phext_bundle_order  /srv/site.order;    # optional: one URL path per line, highest priority first
    phext_bundle_check  1s;                 # re-check interval, off the request path (0 = every request; off)
    phext_bundle_gzip   6;                  # libdeflate 1..12 | off   (default 6)
    phext_bundle_brotli 5;                  # 0..11 | off              (default 5)
    phext_bundle_zstd   3;                  # 1..22 | off              (default 3)
}

location / {
    phext_serve on;                         # /doc.phext;a.b.c;d.e.f;g.h.i
}
```

Each worker builds its bundles at startup, then a per-worker timer re-checks every `phext_bundle_check`, off
the request path. A changed bundle is rebuilt between requests and swapped in. `0` checks on every request
(development); `off` never re-checks. Build cost and throughput: [../bench/results/load.md](../bench/results/load.md).

## Apache httpd

```apache
LoadModule phext_module modules/mod_phext.so

<Location "/app.phext">
    SetHandler          phext-bundle
    PhextBundle         /srv/site
    PhextBundleEntry    index.html
    PhextBundleOrder    /srv/site.order
    PhextBundleCheck    1000              # ms, by a watcher thread (0 = every request; off)
    PhextBundleGzip     6                 # | off
    PhextBundleBrotli   5                 # | off
    PhextBundleZstd     3                 # | off
</Location>

PhextServe On
```

Bundles are per child process, shared by its threads under a mutex. A child builds on its first bundle
request, then a watcher thread re-checks on `PhextBundleCheck`'s interval, off the request path. With
`PhextBundleCheck off`, each child keeps whatever it built first; children started after a change build
the new content. mod_phext sets `no-gzip`, so mod_deflate never recompresses its output.

## Build and test (Linux / WSL)

```bash
servers/build.sh
```

Builds `libphext_server.a`, `ngx_http_phext_module.so` (against the installed nginx's version,
`--with-compat`), `mod_phext.so` (apxs), and the `phext-bundle` CLI into `~/.cache/phext-build`.
It needs cargo, gcc, make, curl, `libpcre3-dev zlib1g-dev libssl-dev apache2-dev`, and nginx.

```bash
servers/test.sh
```

Runs 44 checks against both servers. Every encoding must decode to exactly the CLI's bundle, and the
script also covers encoding choice, `q=0`, 304, 406, HEAD, rebuild on change (with matching ETags across
servers), `phext_serve` lookups, symbol resolution, and clean error logs. It needs `brotli` and `zstd`.

```bash
servers/loadtest.sh spa
```

Runs `h2load` throughput for the modules versus `gzip_static`, plus the cold-build cost.
`servers/nginx-bench.sh start <fixture>` runs nginx on a benchmark fixture for
`node bench/run.mjs --variants phext,phext-nginx` (Chrome end-to-end through the real module).
