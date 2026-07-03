#include <cstdio>
#include <cstdlib>
#include <vector>

#include "Superpowered.h"
#include "SuperpoweredDecoder.h"
#include "metrics.h"

int main(int argc, char **argv) {
    if (argc != 3) {
        std::fprintf(stderr, "usage: sp-dec-bench <path> <rate>\n");
        return 2;
    }
    const char *path = argv[1];
    const unsigned int rate = static_cast<unsigned int>(std::atoi(argv[2]));

    Superpowered::Initialize("ExampleLicenseKey-WillExpire-OnNextUpdate");

    Superpowered::Decoder decoder;
    const Baseline baseline = Baseline::take();
    const int openCode = decoder.open(path);
    if (openCode != Superpowered::Decoder::OpenSuccess) {
        std::fprintf(stderr, "sp-dec-bench open failed: %d (%s)\n", openCode,
                     Superpowered::Decoder::statusCodeToString(openCode));
        return 1;
    }
    const unsigned int fileRate = decoder.getSamplerate();
    if (fileRate != rate) {
        std::fprintf(stderr, "sp-dec-bench rate mismatch: file %u, expected %u\n",
                     fileRate, rate);
        return 1;
    }

    // decodeAudio output must hold numberOfFrames * 4 + 16384 bytes (stereo int16).
    constexpr unsigned int kFrames = 4096;
    std::vector<short> buf(kFrames * 2 + 8192);
    unsigned long long frames = 0;
    double ttfaMs = -1.0;

    while (true) {
        const int decoded = decoder.decodeAudio(buf.data(), kFrames);
        if (decoded > 0) {
            if (ttfaMs < 0.0) {
                ttfaMs = baseline.elapsedMs();
            }
            frames += static_cast<unsigned long long>(decoded);
            continue;
        }
        if (decoded == Superpowered::Decoder::EndOfFile) {
            break;
        }
        std::fprintf(stderr, "sp-dec-bench decode failed: %d\n", decoded);
        return 1;
    }

    printReport(baseline, ttfaMs, frames * 2, rate, 2, "sp-decoder");
    return 0;
}
