// decdrm-xhe-host: a 32-bit shim around Fraunhofer IIS's DRM xHE-AAC/HE-AAC encoder
// DLL (`fhgio.dll` / `fhgxaac.dll`, as shipped with Spark Modulator).
//
// The DLL is 32-bit only and exports nothing but nine JNI entry points, so a 64-bit
// process cannot use it. This host is the 32-bit side of the bridge: it loads the DLL,
// calls the internal DRM+ encoder entry points directly (the JNI wrappers only add
// argument marshalling and an XOR mask over the output), and speaks a line protocol on
// stdin/stdout with raw PCM going in and DRM audio super frames coming out.
//
// Protocol (stdout, binary mode; every line ends with '\n'):
//
//   OK <frameBytes> <nSmpFloats> <pcmRate> <channels> <drmPlus> <sdcHex>
//   NEED <n>            -> the host reads n f32 samples (little-endian) from stdin
//   FRAME <len>         -> followed by len bytes of plain (not XORed) super frame
//   ERROR <code> <msg>  -> the encoder failed; the host exits
//
// `<sdcHex>` is the 5-8 byte SDC fragment the encoder wants (the audio-service part of
// the DRM SDC), as hex. `nSmpFloats` is the number of interleaved f32 samples the encoder
// wants per call; a super frame normally arrives after a few calls.
//
// Modes:
//   --probe     open, print OK, close, exit (a smoke test for the DLL and the parameters)
//   [--bytes N] [--probe|--selftest  feed a 1 kHz sine, check every super frame has the expected size
//   (default)   the stdio protocol above
//
// Licensing: the DLL is a commercial Fraunhofer encoder under its embedded licence blob.
// Neither it nor this host is shipped with DecDRM; build this locally, put `fhgio.dll`
// beside it, and keep both out of the repository.

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <fcntl.h>
#include <io.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// The DRM+ layer inside the DLL, not exported (see refs/dll/fhgio-api.md, section 8.3).
// All __stdcall, 32-bit. RVAs are specific to the build whose SHA-256 is
// 59079D4375F41A765F81DEE2A0E9565FA96542DE9B4EB85A338E9AA23472A53A.
#define RVA_OPEN 0x17E0u
#define RVA_ENCODE 0x1BC0u
#define RVA_CLOSE 0x1B60u

typedef void *enc_t;
typedef int(__stdcall *open_fn)(enc_t *h, int drm_plus, int channels, int bitrate, int *n_smp,
                                unsigned char *sdc, int *n_sdc);
typedef int(__stdcall *encode_fn)(enc_t h, const float *pcm, int n_smp, unsigned char *out, int out_cap,
                                  int *n_bytes, int *n_smp_io, int *samples_left);
typedef int(__stdcall *close_fn)(void **h);

static open_fn g_open;
static encode_fn g_encode;
static close_fn g_close;

static void fail(const char *what, int code) {
    fprintf(stdout, "ERROR %d %s\n", code, what);
    fflush(stdout);
    fprintf(stderr, "decdrm-xhe-host: %s (code %d)\n", what, code);
    exit(1);
}

// `fhgio.dll` beside this executable, unless --dll says otherwise.
static HMODULE load_dll(const char *path) {
    char beside[MAX_PATH];
    HMODULE h = NULL;
    if (path) {
        h = LoadLibraryA(path);
        if (!h) {
            fprintf(stderr, "decdrm-xhe-host: cannot load %s (error %lu)\n", path, GetLastError());
            exit(1);
        }
        return h;
    }
    DWORD n = GetModuleFileNameA(NULL, beside, sizeof beside);
    if (n == 0 || n >= sizeof beside) {
        fprintf(stderr, "decdrm-xhe-host: cannot find my own path\n");
        exit(1);
    }
    char *slash = strrchr(beside, '\\');
    if (slash) *slash = 0;
    for (const char *name = "fhgio.dll";; name = "fhgxaac.dll") {
        char full[MAX_PATH];
        snprintf(full, sizeof full, "%s\\%s", beside, name);
        h = LoadLibraryA(full);
        if (h) return h;
        if (!strcmp(name, "fhgxaac.dll")) break;
    }
    fprintf(stderr, "decdrm-xhe-host: neither fhgio.dll nor fhgxaac.dll is beside the host (error %lu)\n",
            GetLastError());
    exit(1);
}

static void resolve(HMODULE dll) {
    unsigned char *base = (unsigned char *)dll;
    g_open = (open_fn)(void *)(base + RVA_OPEN);
    g_encode = (encode_fn)(void *)(base + RVA_ENCODE);
    g_close = (close_fn)(void *)(base + RVA_CLOSE);
}

// bitrate = bytes * 8 / T, with T = 0.2 s for DRM+ and 0.4 s for DRM30.
static int bitrate_for(int frame_bytes, int drm_plus) {
    return frame_bytes * (drm_plus ? 40 : 20);
}

static void print_ok(int frame_bytes, int n_smp, int channels, int drm_plus, const unsigned char *sdc,
                     int n_sdc) {
    printf("OK %d %d 48000 %d %d ", frame_bytes, n_smp, channels, drm_plus);
    for (int i = 0; i < n_sdc; i++) printf("%02X", sdc[i]);
    printf("\n");
    fflush(stdout);
}

static int open_encoder(const char *dll_path, int drm_plus, int channels, int frame_bytes, enc_t *h, int *n_smp, unsigned char *sdc) {
    int n_sdc = 128;
    int bitrate = bitrate_for(frame_bytes, drm_plus);
    HMODULE dll = load_dll(dll_path);
    resolve(dll);
    memset(sdc, 0, 128);
    int rc = g_open(h, drm_plus, channels, bitrate, n_smp, sdc, &n_sdc);
    if (rc != 0) {
        fprintf(stderr, "decdrm-xhe-host: open(%d, %d, %d) failed: %d\n", drm_plus, channels, bitrate, rc);
        exit(2);
    }
    if (n_sdc < 0 || n_sdc > 128) n_sdc = 128;
    print_ok(frame_bytes, *n_smp, channels, drm_plus, sdc, n_sdc);
    return n_sdc;
}

static void sine_call(enc_t h, float *pcm, int n_smp, int channels, long *pos) {
    (void)h;
    // 1 kHz at 48 kHz, a quarter scale: plenty for a smoke test, no clipping.
    for (int i = 0; i < n_smp; i += channels) {
        float v = (float)(0.25 * sin(2.0 * 3.14159265358979 * 1000.0 * (double)(*pos) / 48000.0));
        for (int c = 0; c < channels; c++) pcm[i + c] = v;
        (*pos)++;
    }
}

static void selftest(enc_t h, int channels, int frame_bytes, int n_smp, int drm_plus, int seconds) {
    unsigned char *out = malloc((size_t)frame_bytes);
    float *pcm = malloc((size_t)n_smp * sizeof(float));
    long pos = 0;
    // One super frame lasts T (0.2 s in DRM+, 0.4 s in DRM30), so this many are wanted.
    long want = seconds * (drm_plus ? 5 : 2) + 1;
    long frames = 0, calls = 0, first_after = -1;
    while (frames < want) {
        int n_bytes = frame_bytes, n_io = n_smp, left = 0;
        sine_call(h, pcm, n_smp, channels, &pos);
        calls++;
        int rc = g_encode(h, pcm, n_smp, out, frame_bytes, &n_bytes, &n_io, &left);
        if (rc != 0) fail("encode failed", rc);
        if (n_bytes > 0) {
            if (first_after < 0) first_after = calls;
            if (n_bytes != frame_bytes) fail("unexpected super frame size", n_bytes);
            if (frames < 3) {
                printf("frame %ld: %d bytes, first 4 %02X %02X %02X %02X\n", frames, n_bytes, out[0], out[1],
                       out[2], out[3]);
            }
            frames++;
        }
    }
    printf("selftest: %ld super frames, first after %ld calls, %ld calls total\n", frames, first_after, calls);
    fflush(stdout);
    free(out);
    free(pcm);
}

int main(int argc, char **argv) {
    const char *dll_path = NULL;
    int drm_plus = 1, channels = 2, frame_bytes = 0, probe = 0, self = 0, seconds = 3;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--dll") && i + 1 < argc) dll_path = argv[++i];
        else if (!strcmp(argv[i], "--drmplus")) drm_plus = 1;
        else if (!strcmp(argv[i], "--drm30")) drm_plus = 0;
        else if (!strcmp(argv[i], "--channels") && i + 1 < argc) channels = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--bytes") && i + 1 < argc) frame_bytes = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--probe")) probe = 1;
        else if (!strcmp(argv[i], "--selftest")) self = 1;
        else if (!strcmp(argv[i], "--seconds") && i + 1 < argc) seconds = atoi(argv[++i]);
        else {
            fprintf(stderr, "usage: decdrm-xhe-host [--dll PATH] [--drmplus|--drm30] [--channels N] "
                            "--bytes N [--probe|--selftest [--seconds N]]\n");
            return 2;
        }
    }
    if (frame_bytes <= 0 || channels < 1 || channels > 2) {
        fprintf(stderr, "decdrm-xhe-host: --bytes and --channels 1|2 are required\n");
        return 2;
    }
    _setmode(_fileno(stdin), _O_BINARY);
    _setmode(_fileno(stdout), _O_BINARY);
    enc_t h = NULL;
    int n_smp = 0;
    unsigned char sdc[128];
    open_encoder(dll_path, drm_plus, channels, frame_bytes, &h, &n_smp, sdc);
    if (probe || self) {
        if (self) selftest(h, channels, frame_bytes, n_smp, drm_plus, seconds);
        g_close(&h);
        return 0;
    }

    unsigned char *out = malloc((size_t)frame_bytes);
    float *pcm = malloc((size_t)n_smp * sizeof(float));
    long unused_total = 0;
    // The encoder often uses only part of the block it is given (`n_io` floats, the rest
    // is not consumed yet). Those samples are audio: they stay in the buffer and are
    // offered again, so `NEED` asks for what is missing rather than for a whole block.
    // Feeding a new block every time instead loses up to a fifth of the audio at the
    // lower bit rates, where the encoder's frame is not a whole number of blocks.
    int have = 0;
    int xhe_debug = getenv("XHE_DEBUG") != NULL;
    for (;;) {
        if (have < n_smp) {
            int want = n_smp - have;
            printf("NEED %d\n", want);
            fflush(stdout);
            if (fread(pcm + have, sizeof(float), (size_t)want, stdin) != (size_t)want) break; // EOF
            have = n_smp;
        }
        int n_bytes = frame_bytes, n_io = n_smp, left = 0;
        int rc = g_encode(h, pcm, n_smp, out, frame_bytes, &n_bytes, &n_io, &left);
        if (rc != 0) fail("encode failed", rc);
        if (xhe_debug)
            fprintf(stderr, "call in %d -> nSmp %d nBytes %d left %d\n", n_smp, n_io, n_bytes, left);
        // `nSmp` is in/out: the encoder says how many of the samples it actually took. It
        // can take none at all (it emits a super frame from what it already holds), and at
        // the lower bit rates it does that often. Whatever it did not take is audio that
        // must be offered again, so it stays in the buffer and the next NEED asks only for
        // what is missing; reading a fresh block over it loses that much of the programme.
        int used = n_io >= 0 && n_io <= n_smp ? n_io : n_smp;
        if (used < n_smp) {
            unused_total += n_smp - used;
            memmove(pcm, pcm + used, (size_t)(n_smp - used) * sizeof(float));
        }
        have = n_smp - used;
        if (n_bytes > 0) {
            printf("FRAME %d\n", n_bytes);
            fwrite(out, 1, (size_t)n_bytes, stdout);
            fflush(stdout);
        }
    }
    fprintf(stderr, "decdrm-xhe-host: %ld samples re-offered instead of being dropped\n", unused_total);
    g_close(&h);
    free(out);
    free(pcm);
    return 0;
}
