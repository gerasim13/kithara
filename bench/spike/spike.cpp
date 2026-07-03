#include <chrono>
#include <cstdio>
#include <cstdlib>

#include "Superpowered.h"
#include "SuperpoweredAdvancedAudioPlayer.h"

int main(int argc, char **argv) {
    if (argc < 4) {
        std::fprintf(stderr, "usage: spike <url> <rate> <tmpdir>\n");
        return 2;
    }

    const char *url = argv[1];
    const unsigned int rate = static_cast<unsigned int>(std::atoi(argv[2]));

    Superpowered::Initialize("ExampleLicenseKey-WillExpire-OnNextUpdate");
    Superpowered::AdvancedAudioPlayer::setTempFolder(argv[3]);

    auto *player = new Superpowered::AdvancedAudioPlayer(rate, 0, 0);
    player->timeStretching = false;
    player->HLSAutomaticAlternativeSwitching = false;
    player->HLSBufferingSeconds = Superpowered::AdvancedAudioPlayer::HLSDownloadRemaining;

    const auto t0 = std::chrono::steady_clock::now();
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
            std::fprintf(stderr, "SPIKE-A FAIL: open error %d (%s)\n", code,
                         Superpowered::AdvancedAudioPlayer::statusCodeToString(code));
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
                ttfaMs = std::chrono::duration<double, std::milli>(
                             std::chrono::steady_clock::now() - t0)
                             .count();
            }
        }

        if (player->eofRecently()) {
            break;
        }

        const double elapsedS =
            std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count();
        if (elapsedS > 300.0) {
            std::fprintf(stderr, "SPIKE-B timeout frames=%llu\n", frames);
            delete player;
            return 1;
        }
    }

    const double wallS =
        std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count();
    std::printf("SPIKE OK: ttfa_ms=%.1f wall_s=%.2f frames=%llu\n", ttfaMs, wallS,
                frames);

    delete player;
    return 0;
}
