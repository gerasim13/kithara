import AVFAudio
import AVFoundation
import Foundation
import Kithara
import Testing

extension IntegrationRegressionsIOS {
    @MainActor
    @Test("Stop then radio playback matches AVQueuePlayer")
    func stopThenRadioPlaybackMatchesAVQueuePlayer() async throws {
        try await TestServerFixture.setNetwork(online: true)
        let audioSession = AVAudioSession.sharedInstance()
        try audioSession.setCategory(.playback)
        try audioSession.setActive(true)
        defer {
            try? audioSession.setActive(false, options: .notifyOthersOnDeactivation)
        }

        let trackURL = try TestServerFixture.signal("signal_mp3_track_sine440_187s.mp3")
        let radioURL = try await TestServerFixture.pacedHlsMasterURL()
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("stop-radio-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )
        defer { try? FileManager.default.removeItem(at: cacheURL) }

        do {
            let player = KitharaPlayer(config: .init(store: AssetStore(root: cacheURL.path)))
            defer { player.stop() }

            try player.insert(KitharaPlayerItem(url: trackURL.absoluteString))
            player.play()
            try await waitForStopParityFact("Kithara track playback") {
                player.currentTime > 0.1
            }

            player.stop()
            try #require(player.itemCount == 0, "Kithara stop left the old queue populated")
            try releaseAndReactivateAudioSession()

            try player.insert(KitharaPlayerItem(url: radioURL.absoluteString))
            player.play()
            try await waitForStopParityFact("Kithara radio playback") {
                player.currentTime > 0.5 && player.currentRate > 0
            }
        }

        let apple = AVQueuePlayer(items: [AVPlayerItem(url: trackURL)])
        defer {
            apple.pause()
            apple.removeAllItems()
        }
        apple.play()
        try await waitForStopParityFact("AVQueuePlayer track playback") {
            finiteStopParitySeconds(apple.currentTime()) > 0.1
        }

        apple.pause()
        apple.removeAllItems()
        try #require(apple.items().isEmpty, "AVQueuePlayer stop left the old queue populated")
        try releaseAndReactivateAudioSession()

        apple.insert(AVPlayerItem(url: radioURL), after: nil)
        apple.play()
        try await waitForStopParityFact("AVQueuePlayer radio playback") {
            finiteStopParitySeconds(apple.currentTime()) > 0.5 && apple.rate > 0
        }
    }

    @MainActor
    private func releaseAndReactivateAudioSession() throws {
        let audioSession = AVAudioSession.sharedInstance()
        try audioSession.setActive(false, options: .notifyOthersOnDeactivation)
        try audioSession.setActive(true)
    }

    @MainActor
    private func waitForStopParityFact(
        _ description: String,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(60))
        while !condition() && clock.now < deadline {
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        try #require(condition(), "Timed out waiting for \(description)")
    }
}

private func finiteStopParitySeconds(_ time: CMTime) -> TimeInterval {
    let seconds = time.seconds
    return seconds.isFinite ? seconds : 0
}
