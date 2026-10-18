import Foundation
import Kithara
import Testing

extension IntegrationRegressionsIOS {
    @Test("A rate change during playback changes how fast media time advances")
    func rateChangeAltersMediaTimeAdvance() async throws {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("rate-change-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )

        let player = KitharaPlayer(config: .init(store: AssetStore(root: cacheURL.path)))
        let item = KitharaPlayerItem(
            url: try TestServerFixture.signal("signal_mp3_track_sine440_187s.mp3").absoluteString
        )
        defer {
            player.stop()
            try? FileManager.default.removeItem(at: cacheURL)
        }

        try player.insert(item)
        player.playingRate = 1
        player.play()
        try await waitForRateFact("fixture playback to advance at rate 1.0") {
            player.currentTime > 0.1
                && abs(player.currentRate - 1) < 0.05
        }

        let normalMeasurement = try await measureMediaTimeAdvance(player)
        try #require(
            normalMeasurement.mediaAdvance > 0,
            "precondition: media time did not advance during the rate-1.0 window"
        )

        // Getters publish applied commands; media advance verifies the live result.
        let requestedRate = 2.0
        player.playingRate = Float(requestedRate)
        try await waitForRateFact("the applied playing rate to be published") {
            abs(Double(player.playingRate) - requestedRate) < 0.05
        }
        try #require(
            abs(Double(player.playingRate) - requestedRate) < 0.05,
            """
            precondition: the player did not publish an applied playing rate of \
            \(requestedRate); it reports \(player.playingRate)
            """
        )
        let fastMeasurement = try await measureMediaTimeAdvance(player)
        #expect(
            abs(Double(player.currentRate) - requestedRate) < 0.05,
            """
            playingRate reports the applied target \(player.playingRate), \
            but currentRate reports the live rate \(player.currentRate)
            """
        )

        let observedRate = fastMeasurement.mediaVelocity / normalMeasurement.mediaVelocity
        let rateTolerance = 0.2
        #expect(
            abs(observedRate - requestedRate) <= rateTolerance,
            """
            requested \(requestedRate)x playback, but normalized media \
            velocity changed by \(observedRate)x; normal window advanced \
            \(normalMeasurement.mediaAdvance)s in \
            \(normalMeasurement.elapsedSeconds)s, fast window advanced \
            \(fastMeasurement.mediaAdvance)s in \
            \(fastMeasurement.elapsedSeconds)s; tolerance is +/-\(rateTolerance)x
            """
        )
    }

    private func measureMediaTimeAdvance(
        _ player: KitharaPlayer
    ) async throws -> MediaTimeMeasurement {
        let clock = ContinuousClock()
        let wallStart = clock.now
        let mediaStart = player.currentTime
        try await Task.sleep(nanoseconds: 2_000_000_000)
        let mediaAdvance = player.currentTime - mediaStart
        let elapsed = wallStart.duration(to: clock.now)
        return MediaTimeMeasurement(mediaAdvance: mediaAdvance, elapsed: elapsed)
    }

    private func waitForRateFact(
        _ description: String,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(30))
        while !condition() {
            guard clock.now < deadline else {
                throw RateFactTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

private struct MediaTimeMeasurement {
    let mediaAdvance: TimeInterval
    let elapsed: Duration

    var elapsedSeconds: TimeInterval {
        elapsed / Duration.seconds(1)
    }

    var mediaVelocity: TimeInterval {
        mediaAdvance / elapsedSeconds
    }
}

private struct RateFactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
