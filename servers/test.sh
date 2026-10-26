#!/usr/bin/env bash
# Functional tests for the nginx and httpd phext modules (run servers/build.sh first).
# Starts each server on high ports in a temp dir, then checks, against the phext-bundle CLI's output:
#   every encoding decodes to the exact same bundle; the smallest accepted encoding wins; Vary/ETag/304/406;
#   rebuild on change; HEAD; and phext_serve coordinate lookups. Needs: curl, gzip, brotli, zstd.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
build="${PHEXT_BUILD:-$HOME/.cache/phext-build}"
work="$(mktemp -d)"
site="$work/site"
fail=0
pids=()
cleanup() {
  [ -f "$work/nginx/nginx.pid" ] && kill "$(cat "$work/nginx/nginx.pid")" 2>/dev/null || true
  [ -f "$work/httpd/httpd.pid" ] && kill "$(cat "$work/httpd/httpd.pid")" 2>/dev/null || true
  sleep 0.3
  rm -rf "$work"
}
trap cleanup EXIT

ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }

echo "== symbols"
for so in ngx_http_phext_module.so mod_phext.so; do
  n="$(nm -D --undefined-only "$build/$so" | grep -c ' phext_' || true)"
  check "$so resolves every phext_ symbol itself" '[ "$n" = 0 ]'
done

# --- a small site -------------------------------------------------------------------------------
mkdir -p "$site/css" "$site/js" "$site/img"
printf '<!doctype html><html><head><link rel=stylesheet href=/css/app.css><script type=module src=/js/main.js></script></head><body><h1>phext</h1><img src=/img/a.bmp></body></html>\n' > "$site/index.html"
for i in $(seq 1 200); do echo ".c$i{margin:${i}px;color:#$((i * 7919 % 999999))}"; done > "$site/css/app.css"
for i in $(seq 1 150); do echo "export const v$i = () => $i * $i; // module code line $i"; done > "$site/js/main.js"
head -c 6000 /dev/urandom > "$site/img/a.bmp"
printf 'scroll one\x17scroll two\x18section two\n' > "$site/doc.phext"
"$build/phext-bundle" "$site" -o "$work/expected/app" --quiet
printf 'ignored precompressed sibling' > "$site/css/app.css.gz"   # the modules must skip this

# --- nginx --------------------------------------------------------------------------------------
mkdir -p "$work/nginx/logs"
cat > "$work/nginx/nginx.conf" <<EOF
load_module $build/ngx_http_phext_module.so;
worker_processes 2;
pid $work/nginx/nginx.pid;
error_log $work/nginx/logs/error.log info;
events {}
http {
  access_log off;
  client_body_temp_path $work/nginx; proxy_temp_path $work/nginx; fastcgi_temp_path $work/nginx;
  uwsgi_temp_path $work/nginx; scgi_temp_path $work/nginx;
  types { application/vnd.phext phext; }
  server {
    listen 127.0.0.1:18080;
    root $site;
    location = /app.phext { phext_bundle $site; phext_bundle_check 0; }
    location = /bg.phext  { phext_bundle $site; phext_bundle_check 200ms; }
    location = /off.phext { phext_bundle $site; phext_bundle_check off; }
    location = /race.phext { phext_bundle $site; phext_bundle_zstd 4; }
    location / { phext_serve on; }
  }
}
EOF
nginx -p "$work/nginx" -c "$work/nginx/nginx.conf" -e "$work/nginx/logs/error.log" 2>/dev/null \
  || nginx -p "$work/nginx" -c "$work/nginx/nginx.conf"

# --- httpd --------------------------------------------------------------------------------------
mods=/usr/lib/apache2/modules
mkdir -p "$work/httpd/logs"
cat > "$work/httpd/httpd.conf" <<EOF
ServerRoot "$work/httpd"
ServerName localhost
Listen 127.0.0.1:18081
PidFile "$work/httpd/httpd.pid"
ErrorLog "$work/httpd/logs/error.log"
LogLevel info
LoadModule mpm_event_module $mods/mod_mpm_event.so
# one child process, so state is deterministic across requests (each child keeps its own bundle)
StartServers 1
ServerLimit 1
ThreadsPerChild 16
MaxRequestWorkers 16
MinSpareThreads 4
MaxSpareThreads 16
LoadModule authz_core_module $mods/mod_authz_core.so
LoadModule mime_module $mods/mod_mime.so
LoadModule phext_module $build/mod_phext.so
TypesConfig /etc/mime.types
AddType application/vnd.phext .phext
DocumentRoot "$site"
<Directory "$site">
  Require all granted
</Directory>
<Location "/app.phext">
  SetHandler phext-bundle
  PhextBundle "$site"
  PhextBundleCheck 0
</Location>
<Location "/bg.phext">
  SetHandler phext-bundle
  PhextBundle "$site"
  PhextBundleCheck 200
</Location>
<Location "/off.phext">
  SetHandler phext-bundle
  PhextBundle "$site"
  PhextBundleCheck off
</Location>
<Location "/race.phext">
  SetHandler phext-bundle
  PhextBundle "$site"
  PhextBundleZstd 4
</Location>
PhextServe On
EOF
apache2 -f "$work/httpd/httpd.conf" -k start
sleep 0.5

# --- checks, per server -------------------------------------------------------------------------
expected="$work/expected/app.phext"
run_checks() {
  local name="$1" base="$2" d="$work/$1"
  mkdir -p "$d"
  echo "== $name ($base)"
  get() { curl -sS --http1.1 -D "$d/$1.h" -o "$d/$1.b" -H "Accept-Encoding: $2" "$base/app.phext"; }
  hdr() { grep -i "^$2:" "$d/$1.h" | head -1 | cut -d' ' -f2- | tr -d '\r'; }

  get id identity
  check "$name identity body == phext-bundle output" 'cmp -s "$d/id.b" "$expected"'
  check "$name content-type" '[ "$(hdr id content-type)" = application/vnd.phext ]'
  check "$name Vary: Accept-Encoding" '[ "$(hdr id vary)" = Accept-Encoding ]'
  for pair in "gzip:gzip -dc" "br:brotli -dc" "zstd:zstd -dcq"; do
    enc="${pair%%:*}" cmd="${pair#*:}"
    get "$enc" "$enc"
    check "$name $enc: Content-Encoding" '[ "$(hdr "$enc" content-encoding)" = "$enc" ]'
    check "$name $enc: decodes to the same bundle" '$cmd < "$d/$enc.b" | cmp -s - "$expected"'
  done
  get all "gzip, deflate, br, zstd"
  smallest="$(for e in gzip br zstd; do echo "$(stat -c %s "$d/$e.b") $e"; done | sort -n | head -1 | cut -d' ' -f2)"
  check "$name picks the smallest accepted ($smallest)" '[ "$(hdr all content-encoding)" = "$smallest" ]'
  get noq "gzip;q=0, br"
  check "$name honors q=0" '[ "$(hdr noq content-encoding)" = br ]'

  etag="$(hdr gzip etag)"
  code="$(curl -s --http1.1 -o /dev/null -w '%{http_code}' -H 'Accept-Encoding: gzip' -H "If-None-Match: $etag" "$base/app.phext")"
  check "$name If-None-Match -> 304" '[ "$code" = 304 ]'
  code="$(curl -s --http1.1 -o /dev/null -w '%{http_code}' -H 'Accept-Encoding: identity;q=0, deflate' "$base/app.phext")"
  check "$name nothing acceptable -> 406" '[ "$code" = 406 ]'
  head_len="$(curl -s --http1.1 -I -H 'Accept-Encoding: gzip' "$base/app.phext" | grep -ic '^content-length:')"
  body_len="$(curl -s --http1.1 -X HEAD -m 2 -o /dev/null -w '%{size_download}' -H 'Accept-Encoding: gzip' "$base/app.phext" || true)"
  check "$name HEAD: headers, no body" '[ "$head_len" = 1 ] && [ "${body_len:-0}" = 0 ]'

  code="$(curl -s --http1.1 -o "$d/sc.b" -w '%{http_code}' "$base/doc.phext;1.1.1;1.1.1;1.1.2")"
  check "$name phext_serve: scroll 1.1.2" '[ "$code" = 200 ] && [ "$(cat "$d/sc.b")" = "scroll two" ]'
  code="$(curl -s --http1.1 -o "$d/sn.b" -w '%{http_code}' "$base/doc.phext;1.1.1;1.1.1;1.2.1")"
  check "$name phext_serve: section 2" '[ "$code" = 200 ] && [ "$(cat "$d/sn.b")" = "section two" ]'
  code="$(curl -s --http1.1 -o /dev/null -w '%{http_code}' "$base/doc.phext;1.1.1;1.1.1;9.9.9")"
  check "$name phext_serve: absent coordinate -> 404" '[ "$code" = 404 ]'
  code="$(curl -s --http1.1 -o /dev/null -w '%{http_code}' "$base/doc.phext;1.1.1;nope")"
  check "$name phext_serve: bad coordinate -> 400" '[ "$code" = 400 ]'
  code="$(curl -s --http1.1 -o "$d/whole.b" -w '%{http_code}' "$base/doc.phext")"
  check "$name plain .phext still served whole" '[ "$code" = 200 ] && cmp -s "$d/whole.b" "$site/doc.phext"'
}

run_checks nginx http://127.0.0.1:18080
run_checks httpd http://127.0.0.1:18081

# a bundle no request has touched yet: 64 concurrent first requests must all succeed (httpd builds on the
# first request in a child; the others have to wait for that build, not fail)
echo "== concurrent first requests"
for pair in nginx:18080 httpd:18081; do
  name="${pair%%:*}" port="${pair#*:}"
  n200="$(seq 1 64 | xargs -P 32 -I{} curl -s --http1.1 -o /dev/null -w '%{http_code}
' "http://127.0.0.1:$port/race.phext" | grep -c '^200$' || true)"
  check "$name: 64 concurrent first requests all 200 ($n200)" '[ "$n200" = 64 ]'
done

# --- rebuild on change -------------------------------------------------------------------------
# /app.phext checks on every request; /bg.phext is re-checked off the request path every 200 ms;
# /off.phext never re-checks.
echo "== rebuild on change"
for port in 18080 18081; do   # prime: httpd creates its per-child state on first request
  for loc in bg off; do curl -s --http1.1 -o /dev/null "http://127.0.0.1:$port/$loc.phext"; done
done
old_n="$(curl -s --http1.1 -H 'Accept-Encoding: gzip' -D - -o /dev/null http://127.0.0.1:18080/app.phext | grep -i '^etag:' | tr -d '\r')"
old_h="$(curl -s --http1.1 -H 'Accept-Encoding: gzip' -D - -o /dev/null http://127.0.0.1:18081/app.phext | grep -i '^etag:' | tr -d '\r')"
sleep 1.1   # mtime granularity
echo ".changed{color:blue}" >> "$site/css/app.css"
rm -f "$site/css/app.css.gz"
"$build/phext-bundle" "$site" -o "$work/expected2/app" --quiet
printf 'ignored precompressed sibling' > "$site/css/app.css.gz"
for pair in nginx:18080 httpd:18081; do
  name="${pair%%:*}" port="${pair#*:}"
  curl -s --http1.1 -H 'Accept-Encoding: identity' -o "$work/$name.new" "http://127.0.0.1:$port/app.phext"
  check "$name serves the rebuilt bundle" 'cmp -s "$work/$name.new" "$work/expected2/app.phext"'
done
new_n="$(curl -s --http1.1 -H 'Accept-Encoding: gzip' -D - -o /dev/null http://127.0.0.1:18080/app.phext | grep -i '^etag:' | tr -d '\r')"
new_h="$(curl -s --http1.1 -H 'Accept-Encoding: gzip' -D - -o /dev/null http://127.0.0.1:18081/app.phext | grep -i '^etag:' | tr -d '\r')"
check "ETags change with the content" '[ "$old_n" != "$new_n" ] && [ "$old_h" != "$new_h" ]'
sleep 0.6   # > the 200 ms check interval plus the httpd watcher's 100 ms tick
for pair in nginx:18080 httpd:18081; do
  name="${pair%%:*}" port="${pair#*:}"
  t="$(curl -s --http1.1 -H 'Accept-Encoding: identity' -o "$work/$name.bg" -w '%{time_total}' "http://127.0.0.1:$port/bg.phext")"
  check "$name background re-check picked up the change (request took ${t}s)" 'cmp -s "$work/$name.bg" "$work/expected2/app.phext"'
  curl -s --http1.1 -H 'Accept-Encoding: identity' -o "$work/$name.off" "http://127.0.0.1:$port/off.phext"
  check "$name check off keeps the original bundle" 'cmp -s "$work/$name.off" "$expected"'
done
check "nginx and httpd agree on the ETag" '[ "$new_n" = "$new_h" ]'

echo "== logs"
check "nginx logged builds, no errors" 'grep -q "phext: built" "$work/nginx/logs/error.log" && ! grep -E "\[(error|crit|alert|emerg)\]" "$work/nginx/logs/error.log"'
check "httpd logged builds, no errors" 'grep -q "phext: built" "$work/httpd/logs/error.log" && ! grep -E ":(error|crit|alert|emerg)\]" "$work/httpd/logs/error.log"'

exit $fail
