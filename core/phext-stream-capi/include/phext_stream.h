/*
 * phext_stream.h - C ABI for the phext-stream Rust crate.
 * Link: libphext_stream_c.a (Unix) or phext_stream_c.lib (MSVC), built from core/phext-stream-capi.
 * Wire format: spec/bundle-v1.md. Keep in sync with core/phext-stream-capi/src/lib.rs.
 *
 * Every pointer argument may be NULL; such calls do nothing (and return an error value where they have
 * one). No function retains a pointer it was given after it returns.
 */
#ifndef PHEXT_STREAM_H
#define PHEXT_STREAM_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PHEXT_DIMENSIONS 9
#define PHEXT_MAX_INDEX 999
/* Longest formatted coordinate is 9 * 3 digits + 8 separators = 35 bytes; 40 always suffices. */
#define PHEXT_COORD_STRLEN 40

/* library, shelf, series, collection, volume, book, chapter, section, scroll; each 1..999 */
typedef struct phext_coord {
    uint16_t d[PHEXT_DIMENSIONS];
} phext_coord;

typedef struct phext_decoder phext_decoder;
typedef struct phext_encoder phext_encoder;

enum {
    PHEXT_SCROLL_START = 1, /* coord set */
    PHEXT_DATA = 2,         /* data/len set; points into the chunk passed to phext_decoder_feed */
    PHEXT_SCROLL_END = 3    /* coord set */
};

typedef void (*phext_event_cb)(void *user, int kind, const phext_coord *coord, const uint8_t *data, size_t len);

const char *phext_version(void);

void phext_coord_origin(phext_coord *out);
/* Parses "a.b.c/d.e.f/g.h.i" or "a.b.c;d.e.f;g.h.i" (len bytes, no NUL needed). 0 on success, -1 on error. */
int phext_coord_parse(const char *s, size_t len, phext_coord *out);
/* snprintf-style: returns the length excluding NUL; writes only if it fits. url != 0 uses ';'. */
size_t phext_coord_format(const phext_coord *c, int url, char *buf, size_t cap);
/* Stream order: <0, 0, >0 */
int phext_coord_cmp(const phext_coord *a, const phext_coord *b);

phext_decoder *phext_decoder_new(void);
void phext_decoder_free(phext_decoder *dec);
/* Calls cb synchronously for every event in this chunk. Never buffers content. */
void phext_decoder_feed(phext_decoder *dec, const uint8_t *data, size_t len, phext_event_cb cb, void *user);
/* Closes the open scroll, if any. */
void phext_decoder_finish(phext_decoder *dec, phext_event_cb cb, void *user);
void phext_decoder_coord(const phext_decoder *dec, phext_coord *out);

phext_encoder *phext_encoder_new(void);
void phext_encoder_free(phext_encoder *enc);
/*
 * Break bytes to move to `next`. Returns their count n. If n <= cap they are written to `out` and the
 * encoder advances; otherwise nothing changes (retry with an n-byte buffer). -1 if `next` is invalid or
 * not strictly after the previous target. Follow with the scroll's content.
 */
int64_t phext_encoder_seek(phext_encoder *enc, const phext_coord *next, uint8_t *out, size_t cap);

/* 0 if data holds no break bytes, else -1 with the first offending offset in *bad_offset. */
int phext_validate(const uint8_t *data, size_t len, size_t *bad_offset);
/* 0 and [*start, *end) of the scroll at target in a complete buffer, or -1 if absent. */
int phext_find(const uint8_t *data, size_t len, const phext_coord *target, size_t *start, size_t *end);

#ifdef __cplusplus
}
#endif

#endif /* PHEXT_STREAM_H */
