#!/usr/bin/env bash
# Load test: the phext modules serving a bundle built on the fly vs. nginx gzip_static serving the same
# bundle pre-built, plus the cost of a (re)build on the request path. Linux/WSL; needs h2load.
# Usage: servers/loadtest.sh [fixture] > bench/results/load.md      (default fixture: spa)
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
build="${PHEXT_BUILD:-$HOME/.cache/phext-build}"
fixture="${1:-spa}"
N="${N:-20000}"
work="$(mktemp -d)"
trap '"$repo/servers/nginx-bench.sh" stop; [ -f "$work/httpd.pid" ] && kill "$(cat "$work/httpd.pid")" 2>/dev/null; sleep 0.3; rm -rf "$work"' EXIT

"$repo/servers/nginx-bench.sh" start "$fixture" >/dev/null
# serve from the local copy nginx-bench.sh made (not /mnt/c under WSL)
out="${PHEXT_NGINX_WORK:-$HOME/.cache/phext-nginx-bench}/site"

mods=/usr/lib/apache2/modules
mkdir -p "$work/logs"
cat > "$work/httpd.conf" <<EOF
ServerRoot "$work"
ServerName localhost
Listen 127.0.0.1:18446
PidFile "$work/httpd.pid"
ErrorLog "$work/logs/error.log"
LoadModule mpm_event_module $mods/mod_mpm_event.so
LoadModule authz_core_module $mods/mod_authz_core.so
LoadModule phext_module $build/mod_phext.so
MaxKeepAliveRequests 0
LogLevel info
DocumentRoot "$out/phext"
<Location "/app.phext">
  SetHandler phext-bundle
  PhextBundle "$out/files"
  PhextBundleOrder "$out/order.txt"
</Location>
EOF
apache2 -f "$work/httpd.conf" -k start
sleep 0.5

# h2load summary -> "req/s | mean latency | p-ish max"
load() {
  local label="$1"; shift
  local o
  o="$(h2load "$@" 2>&1)"
  local rps mbs mean max
  rps="$(sed -n 's/^finished in [^,]*, \([0-9.]*\) req\/s, \([0-9.]*[KMG]*B\/s\).*/\1/p' <<<"$o")"
  mbs="$(sed -n 's/^finished in [^,]*, [0-9.]* req\/s, \([0-9.]*[KMG]*B\/s\).*/\1/p' <<<"$o")"
  # "time for request:  min  max  mean  sd  +/- sd"
  mean="$(grep '^time for request:' <<<"$o" | awk '{print $6}')"
  max="$(grep '^time for request:' <<<"$o" | awk '{print $5}')"
  local okc
  okc="$(sed -n 's/^status codes: \([0-9]*\) 2xx.*/\1/p' <<<"$o")"
  echo "| $label | $rps | $mbs | $mean | $max | $okc / $N |"
}

ae='accept-encoding: gzip, deflate, br, zstd'
echo "# Server module load test: \`$fixture\`"
echo
echo "\`h2load -n $N\`, $(nproc) CPUs (WSL2), nginx $(nginx -v 2>&1 | sed 's|.*/||') with 2 workers, httpd $(apache2 -v | sed -n 's|.*Apache/\([0-9.]*\).*|\1|p') event MPM."
echo "Dynamic = built from \`bench/out/$fixture/files\` by the module and cached in memory (smallest accepted"
echo "encoding: brotli here). Pre-built = the \`phext-bundle\` output served by nginx \`gzip_static\`."
echo
echo "| server / mode | req/s | throughput | mean latency | max latency | 2xx |"
echo "|---|---|---|---|---|---|"
load "nginx, HTTP/2, dynamic (module)"        -n "$N" -c 16 -m 8 -H "$ae" "https://127.0.0.1:18443/app.phext"
load "nginx, HTTP/2, pre-built (gzip_static)" -n "$N" -c 16 -m 8 -H 'accept-encoding: gzip' "https://127.0.0.1:18443/prebuilt.phext"
load "nginx, HTTP/1.1, dynamic (module)"      --h1 -n "$N" -c 16 -H "$ae" "https://127.0.0.1:18444/app.phext"
load "nginx, HTTP/1.1, pre-built (gzip_static)" --h1 -n "$N" -c 16 -H 'accept-encoding: gzip' "https://127.0.0.1:18444/prebuilt.phext"
load "httpd, HTTP/1.1 (plain), dynamic (mod_phext)" --h1 -n "$N" -c 16 -H "$ae" "http://127.0.0.1:18446/app.phext"
load "nginx, HTTP/1.1 (plain), dynamic (module)"  --h1 -n "$N" -c 16 -H "$ae" "http://127.0.0.1:18445/app.phext"

# build cost, as logged by the modules ("phext: built ... in N ms")
size="$(curl -s -H 'Accept-Encoding: identity' http://127.0.0.1:18445/app.phext | wc -c)"
nlog="${PHEXT_NGINX_WORK:-$HOME/.cache/phext-nginx-bench}/logs/error.log"
times() { grep -o 'phext: built .* in [0-9]* ms' "$1" | sed 's/.* in \([0-9]*\) ms/\1/' | sort -n | paste -sd' ' -; }
echo
echo "Build cost for this $size-byte bundle (gzip 6 + br 5 + zstd 3, per process): nginx workers $(times "$nlog")ms;"
echo "httpd children $(times "$work/logs/error.log")ms. nginx workers build at startup; httpd children on their first"
echo "request (concurrent requests wait for it). After that, rebuilds happen off the request path, only when files change."
