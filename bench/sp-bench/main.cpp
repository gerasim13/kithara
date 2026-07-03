#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <thread>

#include "Superpowered.h"
#include "SuperpoweredAdvancedAudioPlayer.h"
#include "metrics.h"

int main(int argc, char **argv) {
    if (argc < 3) {
        std::fprintf(stderr, "usage: sp-bench <url> <rate> [--paced] [--tmp <dir>]\n");
        return 2;
    }

    const char *url = argv[1];
    const unsigned int rate = static_cast<unsigned int>(std::atoi(argv[2]));
    bool paced = false;
    const char *tmp = "/tmp/sp-bench-tmp";

    for (int i = 3; i < argc; i++) {
        if (std::strcmp(argv[i], "--paced") == 0) {
            paced = true;
        } else if (std::strcmp(argv[i], "--tmp") == 0 && i + 1 < argc) {
            tmp = argv[++i];
        } else {
            std::fprintf(stderr, "usage: sp-bench <url> <rate> [--paced] [--tmp <dir>]\n");
            return 2;
        }
    }

    Superpowered::Initialize("ExampleLicenseKey-WillExpire-OnNextUpdate");
    Superpowered::AdvancedAudioPlayer::setTempFolder(tmp);

    auto *player = new Superpowered::AdvancedAudioPlayer(rate, 0, 0);
    player->timeStretching = false;
    player->HLSAutomaticAlternativeSwitching = false;
    player->HLSBufferingSeconds = Superpowered::AdvancedAudioPlayer::HLSDownloadRemaining;

    const Baseline baseline = Baseline::take();
    player->openHLS(url);

    float buf[1024 * 2 + 64];
    bool playing = false;
    bool gotAudio = false;
    unsigned long long frames = 0;
    double ttfaMs = -1.0;

    while (true) {
        const auto ev = player->getLatestEvent();
        if (ev == Superpowered::AdvancedAudioPlayer::PlayerEvent_OpenFailed) {
            const int code = player->getOpenErrorCode();
            std::fprintf(stderr, "sp-bench open failed: %d (%s)\n", code,
                         Superpowered::AdvancedAudioPlayer::statusCodeToString(code));
            delete player;
            return 1;
        }
        if (ev == Superpowered::AdvancedAudioPlayer::PlayerEvent_ConnectionLost) {
            std::fprintf(stderr, "sp-bench connection lost while downloading HLS\n");
            delete player;
            return 1;
        }

        if (!playing && ev == Superpowered::AdvancedAudioPlayer::PlayerEvent_Opened) {
            player->play();
            playing = true;
        }

        player->outputSamplerate = rate;
        const bool has = player->processStereo(buf, false, 1024);
        if (has) {
            frames += 1024;
            if (!gotAudio) {
                gotAudio = true;
                ttfaMs = baseline.elapsedMs();
            }
            if (paced) {
                const double audioPos = static_cast<double>(frames) / static_cast<double>(rate);
                const double wall = baseline.elapsedMs() / 1e3;
                if (audioPos > wall) {
                    std::this_thread::sleep_for(std::chrono::duration<double>(audioPos - wall));
                }
            }
        }

        if (player->eofRecently()) {
            break;
        }

        if (baseline.elapsedMs() > 900000.0) {
            std::fprintf(stderr, "sp-bench timeout frames=%llu\n", frames);
            delete player;
            return 1;
        }
    }

    printReport(baseline, ttfaMs, frames * 2, rate, 2);
    delete player;
    return 0;
}
