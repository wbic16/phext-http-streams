/*
 * phext_server.h - C ABI for web server modules (servers/phext-server).
 * Link: libphext_server.a, which also contains everything in phext_stream.h.
 * Keep in sync with servers/phext-server/src/lib.rs.
 *
 * Model: build a phext_site from a directory once (pre-encoded in every requested Content-Encoding),
 * serve its bodies from memory, and call phext_dir_fingerprint() now and then; when the fingerprint
 * differs from phext_site_fingerprint(), build a new site. A site is immutable: threads may share it,
 * and its bodies stay valid until phext_site_free().
 */
#ifndef PHEXT_SERVER_H
#define PHEXT_SERVER_H

#include <stddef.h>
#include <stdint.h>
#include "phext_stream.h"

#ifdef __cplusplus
extern "C" {
#endif

enum {
    PHEXT_ENC_IDENTITY = 0,
    PHEXT_ENC_GZIP = 1,
    PHEXT_ENC_BR = 2,
    PHEXT_ENC_ZSTD = 3
};

/* Longest ETag phext_site_etag() writes, plus NUL. */
#define PHEXT_ETAG_LEN 32

typedef struct phext_build_opts {
    const char *entry;      /* entry document relative to the directory; NULL = "index.html" */
    const char *order_file; /* priority order, one URL path per line; NULL = default ordering */
    int gzip_level;         /* libdeflate 1..12; 0 = no gzip body */
    int brotli_quality;     /* 0..11; -1 = no br body */
    int zstd_level;         /* 1..22; 0 = no zstd body */
    size_t inline_max;      /* budgeted mode (spec 3.2); SIZE_MAX for both = inline everything */
    size_t flight_budget;
} phext_build_opts;

typedef struct phext_site phext_site;

/* Defaults: inline everything; gzip 6, br 5, zstd 3 (fast enough for a request path). */
void phext_build_opts_default(phext_build_opts *opts);

/* Hash of every bundled file's path, size, and mtime (and the order file's): one directory walk, no
   reads. Precompressed siblings (.gz/.br/.zst) and hidden files are ignored. 0 on error. */
uint64_t phext_dir_fingerprint(const char *dir, const char *order_file);

/* Builds the bundle. 0 and *out on success; -1 with a NUL-terminated message in err otherwise.
   opts may be NULL for the defaults. */
int phext_site_build(const char *dir, const phext_build_opts *opts, phext_site **out, char *err, size_t err_cap);
void phext_site_free(phext_site *site);

uint64_t phext_site_fingerprint(const phext_site *site);
size_t phext_site_entries(const phext_site *site);

/* Body for a PHEXT_ENC_* value, or NULL if that encoding wasn't built. */
const uint8_t *phext_site_body(const phext_site *site, int encoding, size_t *len);
/* Link: preload value for external entries (comma-joined, not NUL-terminated; len 0 when all inline). */
const char *phext_site_links(const phext_site *site, size_t *len);
/* Static NUL-terminated token: "identity", "gzip", "br", "zstd". */
const char *phext_encoding_token(int encoding);
/* Strong ETag with quotes, snprintf-style (returns length, writes if it fits). */
size_t phext_site_etag(const phext_site *site, int encoding, char *buf, size_t cap);
/* Smallest built body the client accepts, from the raw Accept-Encoding value (NULL = header absent).
   -1 if the client refuses all of them (respond 406). */
int phext_site_pick(const phext_site *site, const char *accept_encoding, size_t len);

#ifdef __cplusplus
}
#endif

#endif /* PHEXT_SERVER_H */
