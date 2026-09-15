// LD_PRELOAD: hide VMProtect anti-debug probes and capture a private-key PEM
// the moment the plugin decrypts it into a malloc/BIO buffer.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ptrace.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <unistd.h>

static void *(*real_malloc)(size_t);
static void *(*real_realloc)(void *, size_t);
static void *(*real_calloc)(size_t, size_t);
static void (*real_free)(void *);
static char *(*real_getenv)(const char *);
static int (*real_open)(const char *, int, ...);
static int (*real_openat)(int, const char *, int, ...);
static FILE *(*real_fopen)(const char *, const char *);
static ssize_t (*real_getrandom)(void *, size_t, unsigned int);
static long (*real_ptrace)(enum __ptrace_request, ...);
static void *(*real_bio_new_mem_buf)(const void *, int);

static char g_key_out[512];
static char g_rand_out[512];
static char g_wrap_out[512];
static char g_secret_out[512];
static char g_self_so[512];
static __thread int g_reent;
static pthread_mutex_t g_track_mu = PTHREAD_MUTEX_INITIALIZER;
static struct {
    void *p;
    size_t n;
} g_track[2048];
static struct {
    void *p;
    size_t n;
} g_secret_track[256];
static unsigned g_secret_i;

static void track_secret_add(void *p, size_t n) {
    if (!p || n < 27 || n > 96 || g_secret_out[0] == '\0') {
        return;
    }
    pthread_mutex_lock(&g_track_mu);
    unsigned i = g_secret_i++ % 256;
    g_secret_track[i].p = p;
    g_secret_track[i].n = n;
    pthread_mutex_unlock(&g_track_mu);
}

static size_t track_secret_take(void *p) {
    size_t n = 0;
    if (!p) {
        return 0;
    }
    pthread_mutex_lock(&g_track_mu);
    for (int i = 0; i < 256; i++) {
        if (g_secret_track[i].p == p) {
            n = g_secret_track[i].n;
            g_secret_track[i].p = NULL;
            g_secret_track[i].n = 0;
            break;
        }
    }
    pthread_mutex_unlock(&g_track_mu);
    return n;
}

static void track_add(void *p, size_t n) {
    if (!p || n < 200 || n > 8192) {
        return;
    }
    pthread_mutex_lock(&g_track_mu);
    for (int i = 0; i < 2048; i++) {
        if (g_track[i].p == NULL || g_track[i].p == p) {
            g_track[i].p = p;
            g_track[i].n = n;
            break;
        }
    }
    pthread_mutex_unlock(&g_track_mu);
}

static size_t track_take(void *p) {
    size_t n = 0;
    if (!p) {
        return 0;
    }
    pthread_mutex_lock(&g_track_mu);
    for (int i = 0; i < 2048; i++) {
        if (g_track[i].p == p) {
            n = g_track[i].n;
            g_track[i].p = NULL;
            g_track[i].n = 0;
            break;
        }
    }
    pthread_mutex_unlock(&g_track_mu);
    return n;
}

static void *must_dlsym(const char *name) {
    void *s = dlsym(RTLD_NEXT, name);
    return s;
}

static void strip_env_ld_preload(void) {
    extern char **environ;
    if (!environ) {
        return;
    }
    for (char **e = environ; *e; e++) {
        if (strncmp(*e, "LD_PRELOAD=", 11) == 0 || strncmp(*e, "LD_AUDIT=", 9) == 0) {
            memset(*e, ' ', strlen(*e));
            (*e)[0] = '_';
        }
    }
}

static void write_once(const char *path, const void *buf, size_t n) {
    if (!path || path[0] == '\0' || !buf || n == 0) {
        return;
    }
    int fd = open(path, O_WRONLY | O_CREAT | O_EXCL, 0600);
    if (fd < 0) {
        return;
    }
    ssize_t w = write(fd, buf, n);
    (void)w;
    close(fd);
}

static void capture_secret(const void *buf, size_t n) {
    if (!buf || n < 27 || n > 256 || g_secret_out[0] == '\0' || g_reent) {
        return;
    }
    const char *p = memmem(buf, n, "GLOF", 4);
    if (!p) {
        return;
    }
    size_t left = n - (size_t)(p - (const char *)buf);
    size_t i = 0;
    while (i < left && p[i] >= 0x21 && p[i] <= 0x7e && p[i] != '/') {
        i++;
    }
    if (i < 40 || i > 64) {
        return;
    }
    int dash = 0;
    for (size_t j = 0; j < i; j++) {
        if (p[j] == '-') {
            dash = 1;
            break;
        }
    }
    if (dash) {
        write_once(g_secret_out, p, i);
    }
}

static void capture_pem(const void *buf, size_t n) {
    if (!buf || n < 40 || n > 16 * 1024 || g_reent) {
        return;
    }
    g_reent = 1;
    const char *p = memmem(buf, n, "-----BEGIN PRIVATE KEY-----", 27);
    size_t hdr = 27;
    if (!p) {
        p = memmem(buf, n, "-----BEGIN RSA PRIVATE KEY-----", 31);
        hdr = 31;
    }
    if (!p) {
        p = memmem(buf, n, "-----BEGIN PUBLIC KEY-----", 26);
        hdr = 26;
        if (p && g_wrap_out[0]) {
            const char *end = memmem(p, n - (size_t)(p - (const char *)buf), "-----END ", 9);
            if (end) {
                const char *nl = memchr(end, '\n', (const char *)buf + n - end);
                size_t len = (size_t)((nl ? nl + 1 : end + 32) - p);
                if (len > n - (size_t)(p - (const char *)buf)) {
                    len = n - (size_t)(p - (const char *)buf);
                }
                write_once(g_wrap_out, p, len);
            }
            g_reent = 0;
            return;
        }
        p = memmem(buf, n, "-----BEGIN RSA PUBLIC KEY-----", 30);
        if (p && g_wrap_out[0]) {
            const char *end = memmem(p, n - (size_t)(p - (const char *)buf), "-----END ", 9);
            if (end) {
                const char *nl = memchr(end, '\n', (const char *)buf + n - end);
                size_t len = (size_t)((nl ? nl + 1 : end + 32) - p);
                if (len > n - (size_t)(p - (const char *)buf)) {
                    len = n - (size_t)(p - (const char *)buf);
                }
                write_once(g_wrap_out, p, len);
            }
            g_reent = 0;
            return;
        }
        g_reent = 0;
        return;
    }
    const char *end = memmem(p, n - (size_t)(p - (const char *)buf), "-----END ", 9);
    if (!end) {
        g_reent = 0;
        return;
    }
    const char *nl = memchr(end, '\n', (const char *)buf + n - end);
    size_t len = (size_t)((nl ? nl + 1 : end + 32) - p);
    if (len > n - (size_t)(p - (const char *)buf)) {
        len = n - (size_t)(p - (const char *)buf);
    }
    if (g_key_out[0] == '\0') {
        g_reent = 0;
        return;
    }
    int fd = open(g_key_out, O_WRONLY | O_CREAT | O_EXCL, 0600);
    if (fd < 0) {
        g_reent = 0;
        return;
    }
    (void)hdr;
    ssize_t w = write(fd, p, len);
    (void)w;
    close(fd);
    g_reent = 0;
}

static void capture_rand(const void *buf, size_t n) {
    if (!buf || n < 12 || n > 64 || g_rand_out[0] == '\0' || g_reent) {
        return;
    }
    int fd = open(g_rand_out, O_WRONLY | O_CREAT | O_APPEND, 0600);
    if (fd < 0) {
        return;
    }
    unsigned char rec[65];
    rec[0] = (unsigned char)n;
    memcpy(rec + 1, buf, n);
    ssize_t w = write(fd, rec, n + 1);
    (void)w;
    close(fd);
}

static int is_proc_status(const char *path) {
    return path && (strcmp(path, "/proc/self/status") == 0 ||
                    strcmp(path, "/proc/thread-self/status") == 0);
}

static int is_proc_maps(const char *path) {
    return path && (strcmp(path, "/proc/self/maps") == 0 ||
                    strcmp(path, "/proc/thread-self/maps") == 0);
}

static int spoof_status_fd(void) {
    if (!real_fopen) {
        return -1;
    }
    FILE *in = real_fopen("/proc/self/status", "r");
    if (!in) {
        return -1;
    }
    char tmp[] = "/tmp/bambu-vmp-status-XXXXXX";
    int fd = mkstemp(tmp);
    if (fd < 0) {
        fclose(in);
        return -1;
    }
    unlink(tmp);
    char line[512];
    while (fgets(line, sizeof(line), in)) {
        if (strncmp(line, "TracerPid:", 10) == 0) {
            dprintf(fd, "TracerPid:\t0\n");
        } else {
            dprintf(fd, "%s", line);
        }
    }
    fclose(in);
    lseek(fd, 0, SEEK_SET);
    return fd;
}

static int spoof_maps_fd(void) {
    if (!real_fopen) {
        return -1;
    }
    FILE *in = real_fopen("/proc/self/maps", "r");
    if (!in) {
        return -1;
    }
    char tmp[] = "/tmp/bambu-vmp-maps-XXXXXX";
    int fd = mkstemp(tmp);
    if (fd < 0) {
        fclose(in);
        return -1;
    }
    unlink(tmp);
    char line[768];
    while (fgets(line, sizeof(line), in)) {
        if (g_self_so[0] && strstr(line, g_self_so)) {
            continue;
        }
        if (strstr(line, "libbambu_vmp_hook") || strstr(line, "frida") || strstr(line, "gdb")) {
            continue;
        }
        dprintf(fd, "%s", line);
    }
    fclose(in);
    lseek(fd, 0, SEEK_SET);
    return fd;
}

__attribute__((constructor)) static void hook_init(void) {
    g_reent = 1;
    real_malloc = must_dlsym("malloc");
    real_realloc = must_dlsym("realloc");
    real_calloc = must_dlsym("calloc");
    real_free = must_dlsym("free");
    real_getenv = must_dlsym("getenv");
    real_open = must_dlsym("open");
    real_openat = must_dlsym("openat");
    real_fopen = must_dlsym("fopen");
    real_getrandom = must_dlsym("getrandom");
    real_ptrace = must_dlsym("ptrace");
    real_bio_new_mem_buf = must_dlsym("BIO_new_mem_buf");
    const char *k = real_getenv ? real_getenv("BAMBU_VMP_KEY_OUT") : getenv("BAMBU_VMP_KEY_OUT");
    if (k) {
        snprintf(g_key_out, sizeof(g_key_out), "%s", k);
    }
    const char *r = real_getenv ? real_getenv("BAMBU_VMP_RAND_OUT") : NULL;
    if (r) {
        snprintf(g_rand_out, sizeof(g_rand_out), "%s", r);
    }
    const char *w = real_getenv ? real_getenv("BAMBU_VMP_WRAP_OUT") : NULL;
    if (w) {
        snprintf(g_wrap_out, sizeof(g_wrap_out), "%s", w);
    }
    const char *s = real_getenv ? real_getenv("BAMBU_VMP_SECRET_OUT") : NULL;
    if (s) {
        snprintf(g_secret_out, sizeof(g_secret_out), "%s", s);
    }
    Dl_info info;
    if (dladdr((void *)hook_init, &info) && info.dli_fname) {
        const char *slash = strrchr(info.dli_fname, '/');
        snprintf(g_self_so, sizeof(g_self_so), "%s", slash ? slash + 1 : info.dli_fname);
    }
    strip_env_ld_preload();
    if (real_getenv && real_getenv("BAMBU_VMP_HOOK_VERBOSE")) {
        fprintf(stderr, "bambu-vmp-hook loaded key_out=%s\n", g_key_out[0] ? "yes" : "no");
    }
    g_reent = 0;
}

char *getenv(const char *name) {
    if (!real_getenv) {
        real_getenv = must_dlsym("getenv");
    }
    if (name && (strcmp(name, "LD_PRELOAD") == 0 || strcmp(name, "LD_AUDIT") == 0)) {
        return NULL;
    }
    return real_getenv ? real_getenv(name) : NULL;
}

char *secure_getenv(const char *name) {
    return getenv(name);
}

void *malloc(size_t n) {
    if (!real_malloc) {
        real_malloc = must_dlsym("malloc");
    }
    void *p = real_malloc ? real_malloc(n) : NULL;
    track_add(p, n);
    track_secret_add(p, n);
    return p;
}

void *realloc(void *ptr, size_t n) {
    if (!real_realloc) {
        real_realloc = must_dlsym("realloc");
    }
    size_t old = track_take(ptr);
    size_t olds = track_secret_take(ptr);
    if (old) {
        capture_pem(ptr, old);
        capture_secret(ptr, old);
    }
    if (olds) {
        capture_secret(ptr, olds);
    }
    void *p = real_realloc ? real_realloc(ptr, n) : NULL;
    track_add(p, n);
    track_secret_add(p, n);
    if (p && n >= 200 && n <= 8192) {
        capture_pem(p, n);
    }
    if (p && n >= 27 && n <= 96) {
        capture_secret(p, n);
    }
    return p;
}

void *calloc(size_t a, size_t b) {
    if (!real_calloc) {
        real_calloc = must_dlsym("calloc");
    }
    void *p = real_calloc ? real_calloc(a, b) : NULL;
    size_t n = a * b;
    track_add(p, n);
    track_secret_add(p, n);
    return p;
}

void free(void *ptr) {
    size_t n = track_take(ptr);
    size_t sn = track_secret_take(ptr);
    if (n) {
        capture_pem(ptr, n);
        capture_secret(ptr, n);
    }
    if (sn) {
        capture_secret(ptr, sn);
    }
    if (!real_free) {
        real_free = must_dlsym("free");
    }
    if (real_free) {
        real_free(ptr);
    }
}

long ptrace(enum __ptrace_request request, ...) {
    if (request == PTRACE_TRACEME) {
        return 0;
    }
    va_list ap;
    va_start(ap, request);
    pid_t pid = va_arg(ap, pid_t);
    void *addr = va_arg(ap, void *);
    void *data = va_arg(ap, void *);
    va_end(ap);
    if (!real_ptrace) {
        real_ptrace = must_dlsym("ptrace");
    }
    return real_ptrace ? real_ptrace(request, pid, addr, data) : -1;
}

static int open_spoofed(const char *path, int flags, int mode) {
    if (is_proc_status(path)) {
        int fd = spoof_status_fd();
        if (fd >= 0) {
            return fd;
        }
    }
    if (is_proc_maps(path)) {
        int fd = spoof_maps_fd();
        if (fd >= 0) {
            return fd;
        }
    }
    if (!real_open) {
        real_open = must_dlsym("open");
    }
    if (flags & O_CREAT) {
        return real_open(path, flags, mode);
    }
    return real_open(path, flags);
}

int open(const char *path, int flags, ...) {
    int mode = 0;
    if (flags & O_CREAT) {
        va_list ap;
        va_start(ap, flags);
        mode = va_arg(ap, int);
        va_end(ap);
    }
    return open_spoofed(path, flags, mode);
}

int open64(const char *path, int flags, ...) {
    int mode = 0;
    if (flags & O_CREAT) {
        va_list ap;
        va_start(ap, flags);
        mode = va_arg(ap, int);
        va_end(ap);
    }
    return open_spoofed(path, flags, mode);
}

int openat(int dirfd, const char *path, int flags, ...) {
    int mode = 0;
    if (flags & O_CREAT) {
        va_list ap;
        va_start(ap, flags);
        mode = va_arg(ap, int);
        va_end(ap);
    }
    if (path && path[0] == '/') {
        return open_spoofed(path, flags, mode);
    }
    if (!real_openat) {
        real_openat = must_dlsym("openat");
    }
    if (flags & O_CREAT) {
        return real_openat(dirfd, path, flags, mode);
    }
    return real_openat(dirfd, path, flags);
}

FILE *fopen(const char *path, const char *mode) {
    if (is_proc_status(path) || is_proc_maps(path)) {
        int fd = is_proc_status(path) ? spoof_status_fd() : spoof_maps_fd();
        if (fd >= 0) {
            FILE *f = fdopen(fd, mode ? mode : "r");
            if (f) {
                return f;
            }
            close(fd);
        }
    }
    if (!real_fopen) {
        real_fopen = must_dlsym("fopen");
    }
    return real_fopen(path, mode);
}

ssize_t getrandom(void *buf, size_t buflen, unsigned int flags) {
    if (!real_getrandom) {
        real_getrandom = must_dlsym("getrandom");
    }
    ssize_t n = real_getrandom ? real_getrandom(buf, buflen, flags) : -1;
    if (n > 0) {
        capture_rand(buf, (size_t)n);
    }
    return n;
}

long syscall(long n, ...) {
    static long (*real)(long, ...);
    if (!real) {
        real = must_dlsym("syscall");
    }
    va_list ap;
    va_start(ap, n);
    long a1 = va_arg(ap, long);
    long a2 = va_arg(ap, long);
    long a3 = va_arg(ap, long);
    long a4 = va_arg(ap, long);
    long a5 = va_arg(ap, long);
    long a6 = va_arg(ap, long);
    va_end(ap);
    long rc = real ? real(n, a1, a2, a3, a4, a5, a6) : -1;
    if (n == SYS_getrandom && rc > 0) {
        capture_rand((void *)a1, (size_t)rc);
    }
    return rc;
}

void *BIO_new_mem_buf(const void *buf, int len) {
    size_t n = len < 0 && buf ? strlen(buf) : (size_t)(len < 0 ? 0 : len);
    capture_pem(buf, n);
    capture_secret(buf, n);
    if (!real_bio_new_mem_buf) {
        real_bio_new_mem_buf = must_dlsym("BIO_new_mem_buf");
    }
    if (!real_bio_new_mem_buf) {
        return NULL;
    }
    return real_bio_new_mem_buf(buf, len);
}

static void *crypto_sym(const char *name) {
    void *s = dlsym(RTLD_NEXT, name);
    if (s) {
        return s;
    }
    static void *crypto;
    if (!crypto) {
        crypto = dlopen("libcrypto.so.3", RTLD_NOW | RTLD_NOLOAD);
        if (!crypto) {
            crypto = dlopen("libcrypto.so.3", RTLD_NOW);
        }
    }
    return crypto ? dlsym(crypto, name) : NULL;
}

static void capture_wrap_der(const void *der, int n) {
    if (der && n >= 270 && n <= 400 && g_wrap_out[0]) {
        write_once(g_wrap_out, der, (size_t)n);
    }
}

int RAND_bytes(unsigned char *buf, int num) {
    static int (*real)(unsigned char *, int);
    if (!real) {
        real = crypto_sym("RAND_bytes");
    }
    int rc = real ? real(buf, num) : -1;
    if (rc == 1 && num > 0) {
        capture_rand(buf, (size_t)num);
    }
    return rc;
}

int RAND_priv_bytes(unsigned char *buf, int num) {
    static int (*real)(unsigned char *, int);
    if (!real) {
        real = crypto_sym("RAND_priv_bytes");
    }
    int rc = real ? real(buf, num) : -1;
    if (rc == 1 && num > 0) {
        capture_rand(buf, (size_t)num);
    }
    return rc;
}

int getentropy(void *buf, size_t buflen) {
    static int (*real)(void *, size_t);
    if (!real) {
        real = must_dlsym("getentropy");
    }
    int rc = real ? real(buf, buflen) : -1;
    if (rc == 0) {
        capture_rand(buf, buflen);
    }
    return rc;
}

int RSA_public_encrypt(int flen, const unsigned char *from, unsigned char *to, void *rsa, int padding) {
    static int (*real)(int, const unsigned char *, unsigned char *, void *, int);
    static int (*i2d)(void *, unsigned char **);
    if (!real) {
        real = crypto_sym("RSA_public_encrypt");
    }
    int rc = real ? real(flen, from, to, rsa, padding) : -1;
    if (flen == 32 && rc == 256 && padding == 1) {
        capture_rand(from, 32);
        if (!i2d) {
            i2d = crypto_sym("i2d_RSA_PUBKEY");
        }
        if (i2d && rsa) {
            unsigned char *der = NULL;
            int n = i2d(rsa, &der);
            capture_wrap_der(der, n);
        }
    }
    return rc;
}

int EVP_PKEY_encrypt(void *ctx, unsigned char *out, size_t *outlen, const unsigned char *in, size_t inlen) {
    static int (*real)(void *, unsigned char *, size_t *, const unsigned char *, size_t);
    static void *(*get0)(const void *);
    static int (*i2d)(const void *, unsigned char **);
    if (!real) {
        real = crypto_sym("EVP_PKEY_encrypt");
    }
    int rc = real ? real(ctx, out, outlen, in, inlen) : 0;
    if (rc == 1 && out && inlen == 32 && outlen && *outlen == 256) {
        capture_rand(in, 32);
        if (!get0) {
            get0 = crypto_sym("EVP_PKEY_CTX_get0_pkey");
        }
        if (!i2d) {
            i2d = crypto_sym("i2d_PUBKEY");
        }
        void *pkey = get0 && ctx ? get0(ctx) : NULL;
        if (i2d && pkey) {
            unsigned char *der = NULL;
            int n = i2d(pkey, &der);
            capture_wrap_der(der, n);
        }
    }
    return rc;
}

int EVP_EncryptUpdate(void *ctx, unsigned char *out, int *outl, const unsigned char *in, int inl) {
    static int (*real)(void *, unsigned char *, int *, const unsigned char *, int);
    if (!real) {
        real = crypto_sym("EVP_EncryptUpdate");
    }
    if (inl >= 40 && inl <= 64) {
        capture_secret(in, (size_t)inl);
    }
    return real ? real(ctx, out, outl, in, inl) : 0;
}
