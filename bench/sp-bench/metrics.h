#pragma once

#include <chrono>
#include <cstdio>
#include <sys/resource.h>

struct Baseline {
    std::chrono::steady_clock::time_point t0;
    double u0;
    double s0;

    static double tv(const timeval &t) {
        return static_cast<double>(t.tv_sec) + static_cast<double>(t.tv_usec) / 1e6;
    }

    static Baseline take() {
        rusage ru{};
        getrusage(RUSAGE_SELF, &ru);
        return {std::chrono::steady_clock::now(), tv(ru.ru_utime), tv(ru.ru_stime)};
    }

    double elapsedMs() const {
        return std::chrono::duration<double, std::milli>(
                   std::chrono::steady_clock::now() - t0)
            .count();
    }
};

inline void printReport(const Baseline &baseline, double ttfaMs,
                        unsigned long long samples, unsigned int samplerate,
                        unsigned int channels, const char *decoder) {
    rusage ru{};
    getrusage(RUSAGE_SELF, &ru);
    const double cpuUser = Baseline::tv(ru.ru_utime) - baseline.u0;
    const double cpuSys = Baseline::tv(ru.ru_stime) - baseline.s0;

    std::printf(
        "{\"engine\":\"superpowered\",\"decoder\":\"%s\","
        "\"ttfa_ms\":%.2f,\"wall_ms\":%.2f,"
        "\"cpu_user_s\":%.4f,\"cpu_sys_s\":%.4f,"
        "\"cpu_total_user_s\":%.4f,\"cpu_total_sys_s\":%.4f,"
        "\"max_rss_bytes\":%ld,\"samples\":%llu,"
        "\"pcm_frames\":%llu,\"samplerate\":%u,\"channels\":%u}\n",
        decoder, ttfaMs, baseline.elapsedMs(), cpuUser, cpuSys,
        Baseline::tv(ru.ru_utime), Baseline::tv(ru.ru_stime), ru.ru_maxrss,
        samples, samples / channels, samplerate, channels);
}
