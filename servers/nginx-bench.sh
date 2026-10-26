#!/usr/bin/env bash
# Runs real nginx with the phext module on a benchmark fixture, for bench/run.mjs (variant `phext-nginx`)
# and for load tests. Linux/WSL; run servers/build.sh and node bench/fixtures.mjs first.
#
#   servers/nginx-bench.sh start <fixture>   # HTTPS: 18443 = HTTP/2, 18444 = HTTP/1.1; plain HTTP: 18445
#   servers/nginx-bench.sh stop
#
# /app.phext is built on the fly by phext_bundle from a local copy of bench/out/<fixture>/files (the same
# sources the pre-built bundles come from, in the same priority order); everything else, including the
# Service Worker, is served statically from the copy of bench/out/<fixture>/phext.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
build="${PHEXT_BUILD:-$HOME/.cache/phext-build}"
work="${PHEXT_NGINX_WORK:-$HOME/.cache/phext-nginx-bench}"

case "${1:-}" in
  stop)
    [ -f "$work/nginx.pid" ] && kill "$(cat "$work/nginx.pid")" 2>/dev/null || true
    exit 0 ;;
  start) ;;
  *) echo "usage: $0 start <fixture> | stop" >&2; exit 2 ;;
esac

fixture="${2:?fixture name}"
src="$repo/bench/out/$fixture"
[ -d "$src/files" ] || { echo "missing $src; run node bench/fixtures.mjs" >&2; exit 1; }
"$0" stop
sleep 0.3
# serve from the local filesystem, as a real deployment would: under WSL, /mnt/c is a 9P share where one
# stat walk of a 100-file directory takes ~600 ms (vs 0.15 ms on ext4)
out="$work/site"
rm -rf "$out" && mkdir -p "$out"
cp -r "$src/files" "$src/phext" "$src/order.txt" "$out/"
mkdir -p "$work/logs"
: > "$work/logs/error.log"
cat > "$work/nginx.conf" <<EOF
load_module $build/ngx_http_phext_module.so;
worker_processes 2;
pid $work/nginx.pid;
error_log $work/logs/error.log info;
events { worker_connections 1024; }
http {
  access_log off;
  client_body_temp_path $work; proxy_temp_path $work; fastcgi_temp_path $work; uwsgi_temp_path $work; scgi_temp_path $work;
  include /etc/nginx/mime.types;
  types { application/vnd.phext phext; }
  sendfile on;
  tcp_nodelay on;
  # let load generators reuse connections (defaults close them after 1000 HTTP/2 / 100 HTTP/1.1 requests)
  http2_max_requests 1000000;
  keepalive_requests 1000000;
  ssl_certificate     $repo/bench/certs/cert.pem;
  ssl_certificate_key $repo/bench/certs/key.pem;
  ssl_protocols TLSv1.3;
  server {
    listen 0.0.0.0:18443 ssl http2;
    listen 0.0.0.0:18444 ssl;
    listen 0.0.0.0:18445;
    root $out/phext;
    gzip_static on;
    add_header Cache-Control "max-age=300";
    location = /app.phext {
      phext_bundle       $out/files;
      phext_bundle_order $out/order.txt;
    }
    # the same bundle, pre-built by phext-bundle and served by gzip_static (load-test comparison)
    location = /prebuilt.phext {
      alias $out/phext/app.phext;
      default_type application/vnd.phext;
    }
  }
  # control for bench/run.mjs variant phext-nginx-static: same nginx, pre-built bundle as a static file
  server {
    listen 0.0.0.0:18453 ssl http2;
    listen 0.0.0.0:18454 ssl;
    root $out/phext;
    gzip_static on;
    add_header Cache-Control "max-age=300";
    location = /app.phext { default_type application/vnd.phext; }
  }
}
EOF
nginx -p "$work" -c "$work/nginx.conf" 2>/dev/null || nginx -p "$work" -c "$work/nginx.conf"
echo "nginx serving $fixture (pid $(cat "$work/nginx.pid"))"
