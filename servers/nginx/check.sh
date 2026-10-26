#!/usr/bin/env bash
# Runs stock nginx against a built fixture and checks the bundle is served gzip'd with the phext type.
# Usage (Linux/WSL): servers/nginx/check.sh [site-dir]   default: bench/out/small/phext
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
site="$(cd "${1:-$repo/bench/out/small/phext}" && pwd)"
work="$(mktemp -d)"
trap 'nginx -p "$work" -c "$work/nginx.conf" -s stop 2>/dev/null || true; rm -rf "$work"' EXIT

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 -subj /CN=localhost \
  -keyout "$work/key.pem" -out "$work/cert.pem" 2>/dev/null
sed 's/^/add_header Link "/; s/$/" always;/' "$site/app.links" > "$work/links.conf"
mkdir -p "$work/logs"
{
  echo "worker_processes 1; pid $work/nginx.pid; error_log $work/logs/error.log;"
  echo "events {}"
  echo "http { access_log off; include /etc/nginx/mime.types; default_type application/octet-stream;"
  echo "  client_body_temp_path $work; proxy_temp_path $work; fastcgi_temp_path $work; uwsgi_temp_path $work; scgi_temp_path $work;"
  sed -e "s|PHEXT_ROOT|$site|; s|PHEXT_CERT|$work/cert.pem|; s|PHEXT_KEY|$work/key.pem|; s|PHEXT_LINKS|$work/links.conf|" \
    "$repo/servers/nginx/phext-static.conf"
  echo "}"
} > "$work/nginx.conf"
nginx -p "$work" -c "$work/nginx.conf"

fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
for proto in --http2 --http1.1; do
  hdr="$(curl -sk $proto -H 'Accept-Encoding: gzip' -D - -o "$work/body" https://localhost:8443/app.phext)"
  echo "--- $proto"
  check "status 200"                 '[[ "$hdr" =~ \ 200 ]]'
  check "content-type phext"         'grep -qi "^content-type: application/vnd.phext" <<<"$hdr"'
  check "content-encoding gzip"      'grep -qi "^content-encoding: gzip" <<<"$hdr"'
  check "body is the pre-built .gz"  'cmp -s "$work/body" "$site/app.phext.gz"'
  check "Link preload for externals" '[[ ! -s "$site/app.links" ]] || grep -qi "^link: <" <<<"$hdr"'
done
sw="$(curl -sk -D - -o /dev/null https://localhost:8443/phext-sw.js)"
check "service worker served as JS" 'grep -qi "^content-type: .*javascript" <<<"$sw"'
exit $fail
