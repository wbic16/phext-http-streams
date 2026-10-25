/* Exercises phext_stream.h the way the nginx/httpd modules will. Run via tests/c/run.ps1 (MSVC) or:
 *   cargo build --release -p phext-stream-capi
 *   cc -I core/phext-stream-capi/include core/phext-stream-capi/tests/c/test_capi.c \
 *      target/release/libphext_stream_c.a -lpthread -ldl -lm -o test_capi && ./test_capi
 */
#include <stdio.h>
#include <string.h>
#include "phext_stream.h"

static int failures = 0;
#define CHECK(cond) do { if (!(cond)) { printf("FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond); failures++; } } while (0)

typedef struct {
    int starts, ends;
    phext_coord coords[8];
    char content[8][64];
    size_t lens[8];
} collected;

static void on_event(void *user, int kind, const phext_coord *coord, const uint8_t *data, size_t len) {
    collected *c = (collected *)user;
    if (kind == PHEXT_SCROLL_START) {
        c->coords[c->starts] = *coord;
        c->lens[c->starts] = 0;
        c->starts++;
    } else if (kind == PHEXT_DATA) {
        int i = c->starts - 1;
        memcpy(c->content[i] + c->lens[i], data, len);
        c->lens[i] += len;
        c->content[i][c->lens[i]] = 0;
    } else if (kind == PHEXT_SCROLL_END) {
        CHECK(phext_coord_cmp(coord, &c->coords[c->starts - 1]) == 0);
        c->ends++;
    }
}

int main(void) {
    const char *addrs[4] = { "1.1.1/1.1.1/1.1.1", "1.1.1/1.1.1/1.1.2", "1.1.1/1.1.1/1.2.1", "2.1.1;1.1.1;1.1.3" };
    const char *texts[4] = { "phext-bundle 1\n", "<html>hello</html>", "body{color:red}", "deep scroll" };
    uint8_t stream[512];
    size_t n = 0;
    phext_coord c;
    char buf[PHEXT_COORD_STRLEN];

    printf("phext-stream %s\n", phext_version());

    /* parse / format round trip, both forms */
    CHECK(phext_coord_parse(addrs[3], strlen(addrs[3]), &c) == 0);
    CHECK(phext_coord_format(&c, 0, buf, sizeof buf) == 17 && strcmp(buf, "2.1.1/1.1.1/1.1.3") == 0);
    CHECK(phext_coord_format(&c, 1, buf, sizeof buf) == 17 && strcmp(buf, "2.1.1;1.1.1;1.1.3") == 0);
    CHECK(phext_coord_format(&c, 0, buf, 4) == 17); /* too small: reports needed length */
    CHECK(phext_coord_parse("1.1.1/1.1.1", 11, &c) == -1);
    CHECK(phext_coord_parse("1.1.1/1.1.1/1.1.0", 17, &c) == -1);
    CHECK(phext_coord_parse(NULL, 0, &c) == -1);

    /* encode four scrolls */
    phext_encoder *enc = phext_encoder_new();
    for (int i = 0; i < 4; i++) {
        CHECK(phext_coord_parse(addrs[i], strlen(addrs[i]), &c) == 0);
        int64_t need = phext_encoder_seek(enc, &c, NULL, 0);
        CHECK(need >= 0);
        if (need > 0) {
            /* first call with cap 0 must not advance */
            CHECK(phext_encoder_seek(enc, &c, stream + n, sizeof stream - n) == need);
        }
        n += (size_t)need;
        CHECK(phext_validate((const uint8_t *)texts[i], strlen(texts[i]), NULL) == 0);
        memcpy(stream + n, texts[i], strlen(texts[i]));
        n += strlen(texts[i]);
    }
    /* going backwards is rejected */
    CHECK(phext_encoder_seek(enc, &c, stream, sizeof stream) == -1);
    phext_encoder_free(enc);

    /* expected wire: header SC html SN css LB SC SC deep */
    CHECK(stream[15] == 0x17);
    CHECK(stream[15 + 1 + 18] == 0x18);
    CHECK(stream[15 + 1 + 18 + 1 + 15] == 0x01);

    /* decode one byte at a time: worst-case chunking */
    collected got;
    memset(&got, 0, sizeof got);
    phext_decoder *dec = phext_decoder_new();
    for (size_t i = 0; i < n; i++) {
        phext_decoder_feed(dec, stream + i, 1, on_event, &got);
    }
    phext_decoder_finish(dec, on_event, &got);
    phext_decoder_free(dec);
    CHECK(got.starts == 4 && got.ends == 4);
    for (int i = 0; i < 4 && i < got.starts; i++) {
        phext_coord want;
        phext_coord_parse(addrs[i], strlen(addrs[i]), &want);
        CHECK(phext_coord_cmp(&got.coords[i], &want) == 0);
        CHECK(strcmp(got.content[i], texts[i]) == 0);
    }

    /* random access */
    size_t s = 0, e = 0;
    phext_coord_parse(addrs[2], strlen(addrs[2]), &c);
    CHECK(phext_find(stream, n, &c, &s, &e) == 0 && e - s == 15 && memcmp(stream + s, texts[2], 15) == 0);
    phext_coord_parse("1.1.1/1.1.1/1.1.9", 17, &c);
    CHECK(phext_find(stream, n, &c, &s, &e) == -1);

    /* validation catches delimiter bytes */
    size_t bad = 0;
    const uint8_t png_ish[] = { 0x89, 'P', 'N', 'G', 0x1A, 0x0A };
    CHECK(phext_validate(png_ish, sizeof png_ish, &bad) == -1 && bad == 4);

    /* NULL tolerance */
    phext_decoder_feed(NULL, stream, n, on_event, &got);
    phext_decoder_free(NULL);

    printf(failures ? "%d FAILED\n" : "all C ABI checks passed\n", failures);
    return failures != 0;
}
