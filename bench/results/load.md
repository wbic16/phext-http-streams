# Server module load test: `spa`

`h2load -n 20000`, 16 CPUs (WSL2), nginx 1.18.0 (Ubuntu) with 2 workers, httpd 2.4.52 event MPM.
Dynamic = built from `bench/out/spa/files` by the module and cached in memory (smallest accepted
encoding: brotli here). Pre-built = the `phext-bundle` output served by nginx `gzip_static`.

| server / mode | req/s | throughput | mean latency | max latency | 2xx |
|---|---|---|---|---|---|
| nginx, HTTP/2, dynamic (module) | 11838.82 | 2.05GB/s | 7.62ms | 65.04ms | 20000 / 20000 |
| nginx, HTTP/2, pre-built (gzip_static) | 11080.04 | 1.83GB/s | 9.31ms | 138.55ms | 20000 / 20000 |
| nginx, HTTP/1.1, dynamic (module) | 11081.35 | 1.92GB/s | 1.40ms | 8.09ms | 20000 / 20000 |
| nginx, HTTP/1.1, pre-built (gzip_static) | 10351.84 | 1.71GB/s | 1.48ms | 7.65ms | 20000 / 20000 |
| httpd, HTTP/1.1 (plain), dynamic (mod_phext) | 12615.84 | 2.18GB/s | 1.18ms | 21.72ms | 20000 / 20000 |
| nginx, HTTP/1.1 (plain), dynamic (module) | 17127.75 | 2.96GB/s | 870us | 5.10ms | 20000 / 20000 |

Build cost for this 394072-byte bundle (gzip 6 + br 5 + zstd 3, per process): nginx workers 13 13ms;
httpd children 17 17 18ms. nginx workers build at startup; httpd children on their first
request (concurrent requests wait for it). After that, rebuilds happen off the request path, only when files change.
