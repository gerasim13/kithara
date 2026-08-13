import AVFAudio
import AVFoundation
import Foundation
import Kithara
import Testing

extension IntegrationRegressionsIOS {
    @MainActor
    @Test("A track-to-radio transition matches AVQueuePlayer")
    func trackToRadioTransitionMatchesAVQueuePlayer() async throws {
        try await TestServerFixture.setNetwork(online: true)
        defer {
            Task {
                _ = try? await TestServerFixture.setNetwork(online: true)
            }
        }

        let audioSession = AVAudioSession.sharedInstance()
        try audioSession.setCategory(.playback)
        try audioSession.setActive(true)
        defer {
            try? audioSession.setActive(false, options: .notifyOthersOnDeactivation)
        }

        let trackURL = try TestServerFixture.asset("test.mp3")
        let radioURL = try await TestServerFixture.pacedHlsMasterURL()
        let kithara = try await observeKitharaTrackToRadio(
            trackURL: trackURL,
            radioURL: radioURL
        )
        let apple = try await observeAVQueuePlayerTrackToRadio(
            trackURL: trackURL,
            radioURL: radioURL
        )

        let expected = TrackToRadioTrace(
            trackAdvanced: true,
            queueCleared: true,
            sessionReleased: true,
            radioCurrent: true,
            radioAdvanced: true
        )
        #expect(kithara == expected, "Kithara transition trace was \(kithara)")
        #expect(apple == expected, "AVQueuePlayer transition trace was \(apple)")
        #expect(kithara == apple)
    }

    @MainActor
    private func observeKitharaTrackToRadio(
        trackURL: URL,
        radioURL: URL
    ) async throws -> TrackToRadioTrace {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("track-radio-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )
        let player = KitharaPlayer(
            config: .init(store: AssetStore(root: cacheURL.path))
        )
        defer {
            player.stop()
            try? FileManager.default.removeItem(at: cacheURL)
        }

        let track = KitharaPlayerItem(url: trackURL.absoluteString)
        try player.insert(track)
        player.play()
        try await waitForTrackToRadioFact("Kithara track playback to advance") {
            player.currentTime > 0.1 && player.currentAudioItem === track
        }
        let trackAdvanced = player.currentTime > 0.1

        player.stop()
        let queueCleared = player.itemCount == 0 && player.currentAudioItem == nil
        try #require(
            queueCleared,
            "Kithara stop left the prior track in its queue"
        )
        let sessionReleased = try releaseAndReactivateAudioSession()

        let radio = KitharaPlayerItem(url: radioURL.absoluteString)
        try player.insert(radio)
        player.play()
        try await waitForTrackToRadioFact("Kithara radio playback to advance") {
            player.currentAudioItem === radio && player.currentTime > 0.5
        }

        return TrackToRadioTrace(
            trackAdvanced: trackAdvanced,
            queueCleared: queueCleared,
            sessionReleased: sessionReleased,
            radioCurrent: player.currentAudioItem === radio,
            radioAdvanced: player.currentTime > 0.5 && player.currentRate > 0
        )
    }

    @MainActor
    private func observeAVQueuePlayerTrackToRadio(
        trackURL: URL,
        radioURL: URL
    ) async throws -> TrackToRadioTrace {
        let track = AVPlayerItem(url: trackURL)
        let player = AVQueuePlayer(items: [track])
        defer {
            player.pause()
            player.removeAllItems()
        }

        player.play()
        try await waitForTrackToRadioFact("AVQueuePlayer track playback to advance") {
            finiteSeconds(player.currentTime()) > 0.1 && player.currentItem === track
        }
        let trackAdvanced = finiteSeconds(player.currentTime()) > 0.1

        player.pause()
        player.removeAllItems()
        let queueCleared = player.items().isEmpty && player.currentItem == nil
        try #require(
            queueCleared,
            "AVQueuePlayer clear left the prior track in its queue"
        )
        let sessionReleased = try releaseAndReactivateAudioSession()

        let radio = AVPlayerItem(url: radioURL)
        player.insert(radio, after: nil)
        player.play()
        try await waitForTrackToRadioFact("AVQueuePlayer radio playback to advance") {
            player.currentItem === radio && finiteSeconds(player.currentTime()) > 0.5
        }

        return TrackToRadioTrace(
            trackAdvanced: trackAdvanced,
            queueCleared: queueCleared,
            sessionReleased: sessionReleased,
            radioCurrent: player.currentItem === radio,
            radioAdvanced: finiteSeconds(player.currentTime()) > 0.5 && player.rate > 0
        )
    }

    @MainActor
    private func releaseAndReactivateAudioSession() throws -> Bool {
        let audioSession = AVAudioSession.sharedInstance()
        try audioSession.setActive(false, options: .notifyOthersOnDeactivation)
        try audioSession.setActive(true)
        return true
    }

    @MainActor
    private func waitForTrackToRadioFact(
        _ description: String,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(60))
        while !condition() {
            guard clock.now < deadline else {
                throw TrackToRadioTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

private struct TrackToRadioTrace: Equatable, CustomStringConvertible {
    let trackAdvanced: Bool
    let queueCleared: Bool
    let sessionReleased: Bool
    let radioCurrent: Bool
    let radioAdvanced: Bool

    var description: String {
        "trackAdvanced=\(trackAdvanced), queueCleared=\(queueCleared), "
            + "sessionReleased=\(sessionReleased), radioCurrent=\(radioCurrent), "
            + "radioAdvanced=\(radioAdvanced)"
    }
}

private struct TrackToRadioTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}

private func finiteSeconds(_ time: CMTime) -> TimeInterval {
    let seconds = time.seconds
    return seconds.isFinite ? seconds : 0
}
