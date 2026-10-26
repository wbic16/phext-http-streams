#!/usr/bin/env bash
# Builds the phext server modules on Linux (or WSL):
#   - libphext_server.a       (Rust: bundling, encoders, phext_stream C ABI)
#   - ngx_http_phext_module.so  against the installed nginx's version, --with-compat
#   - mod_phext.so              with apxs
#   - phext-bundle              (the CLI, for tests)
# Output goes to $PHEXT_BUILD (default ~/.cache/phext-build). Needs: cargo, gcc, make, curl,
# libpcre3-dev, zlib1g-dev, libssl-dev, apache2-dev (apxs), and nginx itself for its version.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
out="${PHEXT_BUILD:-$HOME/.cache/phext-build}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/phext-target}"
mkdir -p "$out"

echo "== rust: libphext_server.a, phext-bundle"
cargo build --quiet --release --manifest-path "$repo/Cargo.toml" -p phext-server -p phext-bundle
lib="$CARGO_TARGET_DIR/release/libphext_server.a"
cp "$CARGO_TARGET_DIR/release/phext-bundle" "$out/"
incs=(-I"$repo/servers/phext-server/include" -I"$repo/core/phext-stream-capi/include")

if command -v nginx >/dev/null; then
  ver="$(nginx -v 2>&1 | sed -n 's|.*nginx/\([0-9.]*\).*|\1|p')"
  src="$HOME/.cache/nginx-$ver"
  echo "== nginx $ver: ngx_http_phext_module.so"
  if [ ! -f "$src/configure" ]; then
    curl -fsSL "https://nginx.org/download/nginx-$ver.tar.gz" | tar xz -C "$HOME/.cache"
  fi
  (
    cd "$src"
    PHEXT_LIB="$lib" ./configure --with-compat --with-http_ssl_module --with-http_v2_module \
      --add-dynamic-module="$repo/servers/nginx/module" >/dev/null
    make -s modules >/dev/null
  )
  cp "$src/objs/ngx_http_phext_module.so" "$out/"
fi

if command -v apxs >/dev/null; then
  echo "== httpd: mod_phext.so"
  work="$(mktemp -d)"
  cp "$repo/servers/httpd/mod_phext.c" "$work/"
  (cd "$work" && apxs -c "${incs[@]}" -Wc,-Wall -Wc,-Werror mod_phext.c "$lib" -lpthread -ldl -lm >/dev/null)
  cp "$work/.libs/mod_phext.so" "$out/"
  rm -rf "$work"
fi

ls -la "$out"
