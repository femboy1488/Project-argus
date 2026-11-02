// argus-bench: short speed test for an Argus node (0.3.2).
//
// Token generation in llama.cpp is a matrix x vector product over quantized weights: it reads every weight
// once per token and is limited by memory bandwidth + int8 dot-product speed. This test does the same kind of
// work: int8 rows · int8 vector (NEON sdot when built with dotprod, widening multiply otherwise) over a buffer
// much larger than the CPU caches, on the big cores, for ~1.5 s. Result: effective GB/s of weights processed.
// Higher = more layers per second; the desktop splits layers by this number.
//
// Output (one line of JSON): {"argus":"bench","v":1,"gbps":12.34,"threads":4,"mb":192,"ms":1503,"dotprod":true}
// Usage: argus-bench [-t threads] [-m MB] [-s seconds]
// Static musl build with zig (see build-bench.sh). Runs from `adb shell` (USB) or from the node app (Wi-Fi).
#define _GNU_SOURCE
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#if defined(__aarch64__)
#include <arm_neon.h>
#endif

#define K 4096            // row length (bytes): a typical hidden size
static int8_t *W;         // weights
static int8_t X[K] __attribute__((aligned(64)));
static size_t ROWS;
static volatile int stop_flag = 0;

typedef struct { size_t r0, r1; uint64_t bytes; int64_t sink; } job_t;

static inline int32_t dot_row(const int8_t *w, const int8_t *x) {
#if defined(__aarch64__) && defined(__ARM_FEATURE_DOTPROD)
    int32x4_t a0 = vdupq_n_s32(0), a1 = vdupq_n_s32(0), a2 = vdupq_n_s32(0), a3 = vdupq_n_s32(0);
    for (int i = 0; i < K; i += 64) {
        a0 = vdotq_s32(a0, vld1q_s8(w + i), vld1q_s8(x + i));
        a1 = vdotq_s32(a1, vld1q_s8(w + i + 16), vld1q_s8(x + i + 16));
        a2 = vdotq_s32(a2, vld1q_s8(w + i + 32), vld1q_s8(x + i + 32));
        a3 = vdotq_s32(a3, vld1q_s8(w + i + 48), vld1q_s8(x + i + 48));
    }
    return vaddvq_s32(vaddq_s32(vaddq_s32(a0, a1), vaddq_s32(a2, a3)));
#elif defined(__aarch64__)
    int32x4_t a0 = vdupq_n_s32(0), a1 = vdupq_n_s32(0);
    for (int i = 0; i < K; i += 32) {
        int8x16_t w0 = vld1q_s8(w + i), x0 = vld1q_s8(x + i), w1 = vld1q_s8(w + i + 16), x1 = vld1q_s8(x + i + 16);
        a0 = vpadalq_s16(a0, vmull_s8(vget_low_s8(w0), vget_low_s8(x0)));
        a0 = vpadalq_s16(a0, vmull_high_s8(w0, x0));
        a1 = vpadalq_s16(a1, vmull_s8(vget_low_s8(w1), vget_low_s8(x1)));
        a1 = vpadalq_s16(a1, vmull_high_s8(w1, x1));
    }
    return vaddvq_s32(vaddq_s32(a0, a1));
#else
    int32_t s = 0;
    for (int i = 0; i < K; i++) s += w[i] * x[i];
    return s;
#endif
}

static void *worker(void *p) {
    job_t *j = (job_t *)p;
    while (!stop_flag) {
        for (size_t r = j->r0; r < j->r1 && !stop_flag; r++) {
            j->sink += dot_row(W + r * K, X);
            j->bytes += K;
        }
    }
    return NULL;
}

static double now_s(void) {
    struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec + t.tv_nsec / 1e9;
}

/** Same rule as the node app: cores above the lowest max-frequency tier (big + prime); all if one tier. */
static int big_cores(void) {
    long f[64]; int n = 0;
    for (int i = 0; i < 64; i++) {
        char path[96]; snprintf(path, sizeof path, "/sys/devices/system/cpu/cpu%d/cpufreq/cpuinfo_max_freq", i);
        FILE *fp = fopen(path, "r"); if (!fp) continue;
        if (fscanf(fp, "%ld", &f[n]) == 1) n++;
        fclose(fp);
    }
    if (n == 0) { long c = sysconf(_SC_NPROCESSORS_ONLN); return c > 0 ? (int)c : 4; }
    long lo = f[0]; for (int i = 1; i < n; i++) if (f[i] < lo) lo = f[i];
    int big = 0; for (int i = 0; i < n; i++) if (f[i] > lo) big++;
    return big ? big : n;
}

int main(int argc, char **argv) {
    int threads = 0, mb = 192; double secs = 1.5;
    for (int i = 1; i + 1 < argc; i += 2) {
        if (!strcmp(argv[i], "-t")) threads = atoi(argv[i + 1]);
        else if (!strcmp(argv[i], "-m")) mb = atoi(argv[i + 1]);
        else if (!strcmp(argv[i], "-s")) secs = atof(argv[i + 1]);
    }
    if (threads <= 0) threads = big_cores();
    if (threads > 16) threads = 16;
    if (mb < 32) mb = 32;
    if (mb > 1024) mb = 1024;
    if (secs < 0.3) secs = 0.3;
    if (secs > 10) secs = 10;
    ROWS = ((size_t)mb << 20) / K;
    W = (int8_t *)aligned_alloc(64, ROWS * K);
    if (!W) { printf("{\"argus\":\"bench\",\"v\":1,\"error\":\"no memory\"}\n"); return 1; }
    for (size_t i = 0; i < ROWS * K; i++) W[i] = (int8_t)((i * 2654435761u) >> 24);   // touch every page
    for (int i = 0; i < K; i++) X[i] = (int8_t)(i * 7 - 64);

    job_t jobs[16]; pthread_t th[16];
    size_t per = ROWS / threads;
    for (int t = 0; t < threads; t++) {
        jobs[t].r0 = t * per; jobs[t].r1 = (t == threads - 1) ? ROWS : (t + 1) * per; jobs[t].bytes = 0; jobs[t].sink = 0;
    }
    // warm-up (frequency ramp), not counted
    stop_flag = 0;
    for (int t = 0; t < threads; t++) pthread_create(&th[t], NULL, worker, &jobs[t]);
    struct timespec w = { 0, 250 * 1000000L }; nanosleep(&w, NULL);
    stop_flag = 1;
    for (int t = 0; t < threads; t++) pthread_join(th[t], NULL);

    int64_t sink = 0;
    for (int t = 0; t < threads; t++) { sink += jobs[t].sink; jobs[t].bytes = 0; }
    stop_flag = 0;
    double t0 = now_s();
    for (int t = 0; t < threads; t++) pthread_create(&th[t], NULL, worker, &jobs[t]);
    struct timespec d = { (time_t)secs, (long)((secs - (time_t)secs) * 1e9) }; nanosleep(&d, NULL);
    stop_flag = 1;
    for (int t = 0; t < threads; t++) pthread_join(th[t], NULL);
    double el = now_s() - t0;
    uint64_t bytes = 0;
    for (int t = 0; t < threads; t++) { bytes += jobs[t].bytes; sink += jobs[t].sink; }
#if defined(__ARM_FEATURE_DOTPROD)
    const char *dp = "true";
#else
    const char *dp = "false";
#endif
    printf("{\"argus\":\"bench\",\"v\":1,\"gbps\":%.2f,\"threads\":%d,\"mb\":%d,\"ms\":%d,\"dotprod\":%s,\"chk\":%d}\n",
           bytes / el / 1e9, threads, mb, (int)(el * 1000), dp, (int)(sink & 0xff));
    free(W);
    return 0;
}
