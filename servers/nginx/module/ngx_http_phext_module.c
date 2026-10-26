/*
 * ngx_http_phext_module: serve phext bundles built on the fly from a directory, and single scrolls
 * addressed by coordinate. All phext work happens in libphext_server.a (servers/phext-server).
 *
 *   location = /app.phext {
 *       phext_bundle        /srv/site;          # directory to bundle (inline everything)
 *       phext_bundle_entry  index.html;         # entry document (default index.html)
 *       phext_bundle_order  /srv/site.order;    # optional priority order, one URL path per line
 *       phext_bundle_check  1s;                 # re-check interval, off the request path (0 = on every
 *                                               # request, for development; off = never)
 *       phext_bundle_gzip   6;                  # libdeflate level 1..12, or off
 *       phext_bundle_brotli 5;                  # 0..11, or off
 *       phext_bundle_zstd   3;                  # 1..22, or off
 *   }
 *
 *   location / {
 *       phext_serve on;                         # GET /doc.phext;1.1.1;1.1.1;1.1.3 -> that scroll
 *   }
 *
 * Each worker builds the bundle at startup with every requested Content-Encoding, answers with the
 * smallest one the client accepts, and rebuilds from a timer when the directory's fingerprint (paths,
 * sizes, mtimes) changes, so no request waits on the directory walk. Bodies go out without copying; a
 * request holds a reference, so a rebuild never frees a bundle that is still being sent.
 */

#include <ngx_config.h>
#include <ngx_core.h>
#include <ngx_http.h>

#include "phext_server.h"

#define NGX_HTTP_PHEXT_MAX_FILE (64 * 1024 * 1024)

typedef struct {
    phext_site *site;
    ngx_uint_t  refs;
    ngx_uint_t  stale;
} ngx_http_phext_ref_t;

typedef struct {
    ngx_str_t              dir;
    ngx_str_t              entry;
    ngx_str_t              order;
    ngx_msec_t             check;      /* 0 = check on every request */
    ngx_flag_t             check_off;  /* never check again after the first build */
    ngx_int_t              gzip;
    ngx_int_t              brotli;
    ngx_int_t              zstd;
    ngx_flag_t             serve;

    /* per-worker state: written only after fork, so each worker has its own copy */
    ngx_http_phext_ref_t  *current;
    ngx_event_t           *timer;
} ngx_http_phext_loc_conf_t;

/* every location with phext_bundle, so workers can build at startup and re-check on a timer */
typedef struct {
    ngx_array_t  bundles;   /* of ngx_http_phext_loc_conf_t * */
} ngx_http_phext_main_conf_t;

static void *ngx_http_phext_create_main_conf(ngx_conf_t *cf);
static void *ngx_http_phext_create_loc_conf(ngx_conf_t *cf);
static char *ngx_http_phext_merge_loc_conf(ngx_conf_t *cf, void *parent, void *child);
static char *ngx_http_phext_bundle(ngx_conf_t *cf, ngx_command_t *cmd, void *conf);
static char *ngx_http_phext_check(ngx_conf_t *cf, ngx_command_t *cmd, void *conf);
static char *ngx_http_phext_level(ngx_conf_t *cf, ngx_command_t *cmd, void *conf);
static ngx_int_t ngx_http_phext_init(ngx_conf_t *cf);
static ngx_int_t ngx_http_phext_init_process(ngx_cycle_t *cycle);
static ngx_int_t ngx_http_phext_bundle_handler(ngx_http_request_t *r);

static ngx_command_t ngx_http_phext_commands[] = {

    { ngx_string("phext_bundle"),
      NGX_HTTP_LOC_CONF|NGX_CONF_TAKE1,
      ngx_http_phext_bundle,
      NGX_HTTP_LOC_CONF_OFFSET,
      0,
      NULL },

    { ngx_string("phext_bundle_entry"),
      NGX_HTTP_MAIN_CONF|NGX_HTTP_SRV_CONF|NGX_HTTP_LOC_CONF|NGX_CONF_TAKE1,
      ngx_conf_set_str_slot,
      NGX_HTTP_LOC_CONF_OFFSET,
      offsetof(ngx_http_phext_loc_conf_t, entry),
      NULL },

    { ngx_string("phext_bundle_order"),
      NGX_HTTP_MAIN_CONF|NGX_HTTP_SRV_CONF|NGX_HTTP_LOC_CONF|NGX_CONF_TAKE1,
      ngx_conf_set_str_slot,
      NGX_HTTP_LOC_CONF_OFFSET,
      offsetof(ngx_http_phext_loc_conf_t, order),
      NULL },

    { ngx_string("phext_bundle_check"),
      NGX_HTTP_MAIN_CONF|NGX_HTTP_SRV_CONF|NGX_HTTP_LOC_CONF|NGX_CONF_TAKE1,
      ngx_http_phext_check,
      NGX_HTTP_LOC_CONF_OFFSET,
      0,
      NULL },

    { ngx_string("phext_bundle_gzip"),
      NGX_HTTP_MAIN_CONF|NGX_HTTP_SRV_CONF|NGX_HTTP_LOC_CONF|NGX_CONF_TAKE1,
      ngx_http_phext_level,
      NGX_HTTP_LOC_CONF_OFFSET,
      offsetof(ngx_http_phext_loc_conf_t, gzip),
      NULL },

    { ngx_string("phext_bundle_brotli"),
      NGX_HTTP_MAIN_CONF|NGX_HTTP_SRV_CONF|NGX_HTTP_LOC_CONF|NGX_CONF_TAKE1,
      ngx_http_phext_level,
      NGX_HTTP_LOC_CONF_OFFSET,
      offsetof(ngx_http_phext_loc_conf_t, brotli),
      NULL },

    { ngx_string("phext_bundle_zstd"),
      NGX_HTTP_MAIN_CONF|NGX_HTTP_SRV_CONF|NGX_HTTP_LOC_CONF|NGX_CONF_TAKE1,
      ngx_http_phext_level,
      NGX_HTTP_LOC_CONF_OFFSET,
      offsetof(ngx_http_phext_loc_conf_t, zstd),
      NULL },

    { ngx_string("phext_serve"),
      NGX_HTTP_MAIN_CONF|NGX_HTTP_SRV_CONF|NGX_HTTP_LOC_CONF|NGX_CONF_FLAG,
      ngx_conf_set_flag_slot,
      NGX_HTTP_LOC_CONF_OFFSET,
      offsetof(ngx_http_phext_loc_conf_t, serve),
      NULL },

      ngx_null_command
};

static ngx_http_module_t ngx_http_phext_module_ctx = {
    NULL,                              /* preconfiguration */
    ngx_http_phext_init,               /* postconfiguration */
    ngx_http_phext_create_main_conf,
    NULL,                              /* init main conf */
    NULL, NULL,                        /* server conf */
    ngx_http_phext_create_loc_conf,
    ngx_http_phext_merge_loc_conf
};

ngx_module_t ngx_http_phext_module = {
    NGX_MODULE_V1,
    &ngx_http_phext_module_ctx,
    ngx_http_phext_commands,
    NGX_HTTP_MODULE,
    NULL,                              /* init master */
    NULL,                              /* init module */
    ngx_http_phext_init_process,
    NULL, NULL, NULL, NULL,
    NGX_MODULE_V1_PADDING
};


static void *
ngx_http_phext_create_main_conf(ngx_conf_t *cf)
{
    ngx_http_phext_main_conf_t  *pmcf;

    pmcf = ngx_pcalloc(cf->pool, sizeof(ngx_http_phext_main_conf_t));
    if (pmcf == NULL) {
        return NULL;
    }

    if (ngx_array_init(&pmcf->bundles, cf->pool, 4, sizeof(ngx_http_phext_loc_conf_t *)) != NGX_OK) {
        return NULL;
    }

    return pmcf;
}


static void *
ngx_http_phext_create_loc_conf(ngx_conf_t *cf)
{
    ngx_http_phext_loc_conf_t  *conf;

    conf = ngx_pcalloc(cf->pool, sizeof(ngx_http_phext_loc_conf_t));
    if (conf == NULL) {
        return NULL;
    }

    conf->check = NGX_CONF_UNSET_MSEC;
    conf->check_off = NGX_CONF_UNSET;
    conf->gzip = NGX_CONF_UNSET;
    conf->brotli = NGX_CONF_UNSET;
    conf->zstd = NGX_CONF_UNSET;
    conf->serve = NGX_CONF_UNSET;

    return conf;
}


/* NUL-terminates a configured path (ngx_str_t data is not), resolving it against the prefix. */
static ngx_int_t
ngx_http_phext_cstr(ngx_conf_t *cf, ngx_str_t *s, ngx_uint_t full)
{
    u_char  *p;

    if (s->len == 0) {
        return NGX_OK;
    }

    if (full && ngx_conf_full_name(cf->cycle, s, 0) != NGX_OK) {
        return NGX_ERROR;
    }

    p = ngx_pnalloc(cf->pool, s->len + 1);
    if (p == NULL) {
        return NGX_ERROR;
    }

    ngx_memcpy(p, s->data, s->len);
    p[s->len] = '\0';
    s->data = p;

    return NGX_OK;
}


static char *
ngx_http_phext_merge_loc_conf(ngx_conf_t *cf, void *parent, void *child)
{
    ngx_http_phext_loc_conf_t *prev = parent;
    ngx_http_phext_loc_conf_t *conf = child;

    ngx_conf_merge_str_value(conf->entry, prev->entry, "index.html");
    ngx_conf_merge_str_value(conf->order, prev->order, "");
    ngx_conf_merge_msec_value(conf->check, prev->check, 1000);
    ngx_conf_merge_value(conf->check_off, prev->check_off, 0);
    ngx_conf_merge_value(conf->gzip, prev->gzip, 6);
    ngx_conf_merge_value(conf->brotli, prev->brotli, 5);
    ngx_conf_merge_value(conf->zstd, prev->zstd, 3);
    ngx_conf_merge_value(conf->serve, prev->serve, 0);

    if (conf->dir.len) {
        if (ngx_http_phext_cstr(cf, &conf->entry, 0) != NGX_OK
            || ngx_http_phext_cstr(cf, &conf->order, 1) != NGX_OK)
        {
            return NGX_CONF_ERROR;
        }
    }

    return NGX_CONF_OK;
}


static char *
ngx_http_phext_bundle(ngx_conf_t *cf, ngx_command_t *cmd, void *conf)
{
    ngx_http_phext_loc_conf_t *plcf = conf;

    ngx_str_t                   *value;
    ngx_http_core_loc_conf_t    *clcf;
    ngx_http_phext_loc_conf_t  **slot;
    ngx_http_phext_main_conf_t  *pmcf;

    if (plcf->dir.len) {
        return "is duplicate";
    }

    pmcf = ngx_http_conf_get_module_main_conf(cf, ngx_http_phext_module);
    slot = ngx_array_push(&pmcf->bundles);
    if (slot == NULL) {
        return NGX_CONF_ERROR;
    }
    *slot = plcf;

    value = cf->args->elts;
    plcf->dir = value[1];

    if (ngx_http_phext_cstr(cf, &plcf->dir, 1) != NGX_OK) {
        return NGX_CONF_ERROR;
    }

    clcf = ngx_http_conf_get_module_loc_conf(cf, ngx_http_core_module);
    clcf->handler = ngx_http_phext_bundle_handler;

    return NGX_CONF_OK;
}


/* phext_bundle_check <time> | off */
static char *
ngx_http_phext_check(ngx_conf_t *cf, ngx_command_t *cmd, void *conf)
{
    ngx_http_phext_loc_conf_t *plcf = conf;

    ngx_str_t  *value;

    if (plcf->check != NGX_CONF_UNSET_MSEC || plcf->check_off != NGX_CONF_UNSET) {
        return "is duplicate";
    }

    value = cf->args->elts;
    if (ngx_strcmp(value[1].data, "off") == 0) {
        plcf->check_off = 1;
        plcf->check = 0;
        return NGX_CONF_OK;
    }

    plcf->check_off = 0;
    plcf->check = ngx_parse_time(&value[1], 0);
    if (plcf->check == (ngx_msec_t) NGX_ERROR) {
        return "invalid value";
    }

    return NGX_CONF_OK;
}


/* A compression level, or "off". */
static char *
ngx_http_phext_level(ngx_conf_t *cf, ngx_command_t *cmd, void *conf)
{
    char       *p = conf;
    ngx_int_t  *np, n;
    ngx_str_t  *value;

    np = (ngx_int_t *) (p + cmd->offset);
    if (*np != NGX_CONF_UNSET) {
        return "is duplicate";
    }

    value = cf->args->elts;
    if (ngx_strcmp(value[1].data, "off") == 0) {
        *np = -1;
        return NGX_CONF_OK;
    }

    n = ngx_atoi(value[1].data, value[1].len);
    if (n == NGX_ERROR || n > 22) {
        return "invalid level";
    }

    *np = n;
    return NGX_CONF_OK;
}


static void
ngx_http_phext_release(void *data)
{
    ngx_http_phext_ref_t *ref = data;

    if (--ref->refs == 0 && ref->stale) {
        phext_site_free(ref->site);
        ngx_free(ref);
    }
}


/* Builds or rebuilds the worker's bundle when the directory changed. Keeps serving the previous bundle
   if a rebuild fails. Called at worker start, from the check timer, and on the request path only when
   there is no bundle yet or phext_bundle_check is 0. */
static ngx_int_t
ngx_http_phext_refresh(ngx_http_phext_loc_conf_t *plcf, ngx_log_t *log)
{
    char                   err[512];
    uint64_t               fp;
    phext_site            *site;
    ngx_msec_t             t0;
    phext_build_opts       opts;
    ngx_http_phext_ref_t  *ref, *old;

    ngx_time_update();
    t0 = ngx_current_msec;

    fp = phext_dir_fingerprint((char *) plcf->dir.data,
                               plcf->order.len ? (char *) plcf->order.data : NULL);
    if (fp == 0) {
        ngx_log_error(NGX_LOG_ERR, log, 0, "phext: cannot read \"%V\"", &plcf->dir);
        return plcf->current ? NGX_OK : NGX_ERROR;
    }

    if (plcf->current && phext_site_fingerprint(plcf->current->site) == fp) {
        return NGX_OK;
    }

    phext_build_opts_default(&opts);
    opts.entry = (char *) plcf->entry.data;
    opts.order_file = plcf->order.len ? (char *) plcf->order.data : NULL;
    opts.gzip_level = plcf->gzip < 0 ? 0 : (int) plcf->gzip;
    opts.brotli_quality = (int) plcf->brotli;
    opts.zstd_level = plcf->zstd < 0 ? 0 : (int) plcf->zstd;

    if (phext_site_build((char *) plcf->dir.data, &opts, &site, err, sizeof(err)) != 0) {
        ngx_log_error(NGX_LOG_ERR, log, 0, "%s", err);
        return plcf->current ? NGX_OK : NGX_ERROR;
    }

    ref = ngx_alloc(sizeof(ngx_http_phext_ref_t), log);
    if (ref == NULL) {
        phext_site_free(site);
        return plcf->current ? NGX_OK : NGX_ERROR;
    }
    ref->site = site;
    ref->refs = 1;          /* the worker's own reference */
    ref->stale = 0;

    old = plcf->current;
    plcf->current = ref;
    if (old) {
        old->stale = 1;
        ngx_http_phext_release(old);
    }

    ngx_time_update();
    ngx_log_error(NGX_LOG_INFO, log, 0, "phext: built \"%V\" (%uz entries) in %M ms",
                  &plcf->dir, phext_site_entries(site), ngx_current_msec - t0);
    return NGX_OK;
}


/* Re-checks between requests, so no visitor waits on a directory walk (slow on network filesystems). */
static void
ngx_http_phext_timer_handler(ngx_event_t *ev)
{
    ngx_http_phext_loc_conf_t *plcf = ev->data;

    if (ngx_exiting || ngx_quit || ngx_terminate) {
        return;
    }

    (void) ngx_http_phext_refresh(plcf, ev->log);
    ngx_add_timer(ev, plcf->check);
}


/* Each worker builds its bundles before taking requests, then arms the check timers. */
static ngx_int_t
ngx_http_phext_init_process(ngx_cycle_t *cycle)
{
    ngx_uint_t                   i;
    ngx_http_conf_ctx_t         *ctx;
    ngx_http_phext_loc_conf_t  **bundles, *plcf;
    ngx_http_phext_main_conf_t  *pmcf;

    ctx = (ngx_http_conf_ctx_t *) cycle->conf_ctx[ngx_http_module.index];
    if (ctx == NULL) {
        return NGX_OK;
    }
    pmcf = ctx->main_conf[ngx_http_phext_module.ctx_index];
    if (pmcf == NULL) {
        return NGX_OK;
    }

    bundles = pmcf->bundles.elts;
    for (i = 0; i < pmcf->bundles.nelts; i++) {
        plcf = bundles[i];
        (void) ngx_http_phext_refresh(plcf, cycle->log);

        if (plcf->check_off || plcf->check == 0) {
            continue;
        }

        plcf->timer = ngx_pcalloc(cycle->pool, sizeof(ngx_event_t));
        if (plcf->timer == NULL) {
            return NGX_ERROR;
        }
        plcf->timer->handler = ngx_http_phext_timer_handler;
        plcf->timer->data = plcf;
        plcf->timer->log = cycle->log;
        plcf->timer->cancelable = 1;   /* don't hold up graceful shutdown */
        ngx_add_timer(plcf->timer, plcf->check);
    }

    return NGX_OK;
}


static ngx_table_elt_t *
ngx_http_phext_add_header(ngx_http_request_t *r, char *key, u_char *value, size_t len)
{
    ngx_table_elt_t  *h;

    h = ngx_list_push(&r->headers_out.headers);
    if (h == NULL) {
        return NULL;
    }

    h->hash = 1;
    h->key.data = (u_char *) key;
    h->key.len = ngx_strlen(key);
    h->value.data = value;
    h->value.len = len;
    return h;
}


static ngx_str_t *
ngx_http_phext_request_header(ngx_http_request_t *r, char *name)
{
    size_t            len = ngx_strlen(name);
    ngx_uint_t        i;
    ngx_list_part_t  *part = &r->headers_in.headers.part;
    ngx_table_elt_t  *h = part->elts;

    for (i = 0; /* void */; i++) {
        if (i >= part->nelts) {
            if (part->next == NULL) {
                return NULL;
            }
            part = part->next;
            h = part->elts;
            i = 0;
        }
        if (h[i].key.len == len && ngx_strncasecmp(h[i].key.data, (u_char *) name, len) == 0) {
            return &h[i].value;
        }
    }
}


static ngx_int_t
ngx_http_phext_bundle_handler(ngx_http_request_t *r)
{
    int                         enc;
    size_t                      len, links_len, etag_len;
    u_char                     *etag;
    ngx_int_t                   rc;
    ngx_buf_t                  *b;
    ngx_str_t                  *accept;
    ngx_chain_t                 out;
    const char                 *token, *links;
    const uint8_t              *body;
    ngx_table_elt_t            *h;
    ngx_pool_cleanup_t         *cln;
    ngx_http_phext_ref_t       *ref;
    ngx_http_phext_loc_conf_t  *plcf;

    if (!(r->method & (NGX_HTTP_GET|NGX_HTTP_HEAD))) {
        return NGX_HTTP_NOT_ALLOWED;
    }

    rc = ngx_http_discard_request_body(r);
    if (rc != NGX_OK) {
        return rc;
    }

    plcf = ngx_http_get_module_loc_conf(r, ngx_http_phext_module);
    /* normally built at worker start and re-checked by the timer; the request path only builds when
       there is nothing yet (startup build failed) or when configured to check every request */
    if ((plcf->current == NULL || (plcf->check == 0 && !plcf->check_off))
        && ngx_http_phext_refresh(plcf, r->connection->log) != NGX_OK)
    {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    if (plcf->current == NULL) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    ref = plcf->current;

    accept = ngx_http_phext_request_header(r, "Accept-Encoding");
    enc = phext_site_pick(ref->site, accept ? (char *) accept->data : NULL, accept ? accept->len : 0);

    /* hold the bundle until this request is done with it, whatever path it takes from here */
    cln = ngx_pool_cleanup_add(r->pool, 0);
    if (cln == NULL) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    ref->refs++;
    cln->handler = ngx_http_phext_release;
    cln->data = ref;

    h = ngx_http_phext_add_header(r, "Vary", (u_char *) "Accept-Encoding", sizeof("Accept-Encoding") - 1);
    if (h == NULL) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }

    if (enc < 0) {
        r->headers_out.status = 406;
        r->headers_out.content_length_n = 0;
        r->header_only = 1;
        return ngx_http_send_header(r);
    }

    body = phext_site_body(ref->site, enc, &len);

    ngx_str_set(&r->headers_out.content_type, "application/vnd.phext");
    r->headers_out.content_type_len = r->headers_out.content_type.len;
    r->headers_out.content_type_lowcase = NULL;

    if (enc != PHEXT_ENC_IDENTITY) {
        token = phext_encoding_token(enc);
        h = ngx_http_phext_add_header(r, "Content-Encoding", (u_char *) token, ngx_strlen(token));
        if (h == NULL) {
            return NGX_HTTP_INTERNAL_SERVER_ERROR;
        }
        r->headers_out.content_encoding = h;
    }

    etag = ngx_pnalloc(r->pool, PHEXT_ETAG_LEN);
    if (etag == NULL) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    etag_len = phext_site_etag(ref->site, enc, (char *) etag, PHEXT_ETAG_LEN);
    h = ngx_http_phext_add_header(r, "ETag", etag, etag_len);
    if (h == NULL) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    r->headers_out.etag = h;

    if (ngx_http_phext_add_header(r, "Cache-Control", (u_char *) "no-cache", sizeof("no-cache") - 1) == NULL) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }

    links = phext_site_links(ref->site, &links_len);
    if (links_len
        && ngx_http_phext_add_header(r, "Link", (u_char *) links, links_len) == NULL)
    {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }

    r->headers_out.status = NGX_HTTP_OK;
    r->headers_out.content_length_n = len;

    rc = ngx_http_send_header(r);
    if (rc == NGX_ERROR || rc > NGX_OK || r->header_only) {
        return rc;
    }

    b = ngx_calloc_buf(r->pool);
    if (b == NULL) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }

    b->pos = (u_char *) body;
    b->last = (u_char *) body + len;
    b->memory = 1;
    b->last_buf = (r == r->main) ? 1 : 0;
    b->last_in_chain = 1;

    out.buf = b;
    out.next = NULL;

    return ngx_http_output_filter(r, &out);
}


/* phext_serve: /path/doc.phext;a.b.c;d.e.f;g.h.i -> that scroll. Anything else is declined, so the
   static module still serves whole .phext files. */
static ngx_int_t
ngx_http_phext_serve_handler(ngx_http_request_t *r)
{
    u_char                     *p, *last, *buf;
    size_t                      root, start, end, saved;
    ssize_t                     n;
    ngx_fd_t                    fd;
    ngx_str_t                   path, coord;
    ngx_int_t                   rc;
    ngx_buf_t                  *b;
    ngx_file_t                  file;
    ngx_chain_t                 out;
    phext_coord                 target;
    ngx_file_info_t             fi;
    ngx_http_phext_loc_conf_t  *plcf;

    plcf = ngx_http_get_module_loc_conf(r, ngx_http_phext_module);
    if (!plcf->serve || !(r->method & (NGX_HTTP_GET|NGX_HTTP_HEAD))) {
        return NGX_DECLINED;
    }

    /* find ".phext;" in the decoded URI */
    last = r->uri.data + r->uri.len;
    p = ngx_strlcasestrn(r->uri.data, last, (u_char *) ".phext;", sizeof(".phext;") - 1 - 1);
    if (p == NULL) {
        return NGX_DECLINED;
    }
    p += sizeof(".phext") - 1;
    coord.data = p + 1;
    coord.len = last - coord.data;

    if (phext_coord_parse((char *) coord.data, coord.len, &target) != 0) {
        return NGX_HTTP_BAD_REQUEST;
    }

    rc = ngx_http_discard_request_body(r);
    if (rc != NGX_OK) {
        return rc;
    }

    /* map just the file part of the URI */
    saved = r->uri.len;
    r->uri.len = p - r->uri.data;
    last = ngx_http_map_uri_to_path(r, &path, &root, 0);
    r->uri.len = saved;
    if (last == NULL) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    path.len = last - path.data;

    fd = ngx_open_file(path.data, NGX_FILE_RDONLY, NGX_FILE_OPEN, 0);
    if (fd == NGX_INVALID_FILE) {
        return NGX_HTTP_NOT_FOUND;
    }

    ngx_memzero(&file, sizeof(ngx_file_t));
    file.fd = fd;
    file.name = path;
    file.log = r->connection->log;

    if (ngx_fd_info(fd, &fi) == NGX_FILE_ERROR || !ngx_is_file(&fi)
        || ngx_file_size(&fi) > NGX_HTTP_PHEXT_MAX_FILE)
    {
        ngx_close_file(fd);
        return NGX_HTTP_NOT_FOUND;
    }

    buf = ngx_pnalloc(r->pool, (size_t) ngx_file_size(&fi) + 1);
    if (buf == NULL) {
        ngx_close_file(fd);
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }

    n = ngx_read_file(&file, buf, (size_t) ngx_file_size(&fi), 0);
    ngx_close_file(fd);
    if (n != (ssize_t) ngx_file_size(&fi)) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }

    if (phext_find(buf, (size_t) n, &target, &start, &end) != 0) {
        return NGX_HTTP_NOT_FOUND;
    }

    ngx_str_set(&r->headers_out.content_type, "text/plain; charset=utf-8");
    r->headers_out.content_type_len = sizeof("text/plain") - 1;
    r->headers_out.content_type_lowcase = NULL;
    r->headers_out.status = NGX_HTTP_OK;
    r->headers_out.content_length_n = end - start;

    rc = ngx_http_send_header(r);
    if (rc == NGX_ERROR || rc > NGX_OK || r->header_only) {
        return rc;
    }

    b = ngx_calloc_buf(r->pool);
    if (b == NULL) {
        return NGX_HTTP_INTERNAL_SERVER_ERROR;
    }
    b->pos = buf + start;
    b->last = buf + end;
    b->memory = 1;
    b->last_buf = (r == r->main) ? 1 : 0;
    b->last_in_chain = 1;

    out.buf = b;
    out.next = NULL;
    return ngx_http_output_filter(r, &out);
}


static ngx_int_t
ngx_http_phext_init(ngx_conf_t *cf)
{
    ngx_http_handler_pt        *h;
    ngx_http_core_main_conf_t  *cmcf;

    cmcf = ngx_http_conf_get_module_main_conf(cf, ngx_http_core_module);

    h = ngx_array_push(&cmcf->phases[NGX_HTTP_CONTENT_PHASE].handlers);
    if (h == NULL) {
        return NGX_ERROR;
    }

    *h = ngx_http_phext_serve_handler;
    return NGX_OK;
}
