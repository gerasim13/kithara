import AVFoundation
import Foundation
import Kithara
import Testing

extension IntegrationRegressionsIOS {
    @Test("Rate comparison normalizes unequal wall-clock windows")
    func rateComparisonNormalizesUnequalWallClockWindows() {
        let normal = MediaTimeMeasurement(
            mediaAdvance: 2,
            elapsed: .seconds(2)
        )
        let fast = MediaTimeMeasurement(
            mediaAdvance: 4.8,
            elapsed: .milliseconds(2_400)
        )

        #expect(abs(fast.mediaVelocity / normal.mediaVelocity - 2) < 0.000_001)
    }

    @Test("Playback-rate effect matches AVPlayer")
    func rateChangeMediaTimeMatchesAVPlayer() async throws {
        let fixtureURL = try TestServerFixture.asset("test.mp3")

        let kithara = try await runKitharaRateScenario(fixtureURL)
        let apple = try await runAVPlayerRateScenario(fixtureURL)

        let requestedRate = 2.0
        let rateTolerance = 0.2
        try #require(
            abs(kithara.velocityRatio - requestedRate) <= rateTolerance,
            """
            Kithara requested \(requestedRate)x playback, but normalized media \
            velocity changed by \(kithara.velocityRatio)x
            """
        )
        try #require(
            abs(apple.velocityRatio - requestedRate) <= rateTolerance,
            """
            AVPlayer requested \(requestedRate)x playback, but normalized media \
            velocity changed by \(apple.velocityRatio)x
            """
        )
        #expect(
            abs(kithara.velocityRatio - apple.velocityRatio) <= rateTolerance,
            """
            normalized playback-rate effects diverged: Kithara \
            \(kithara.velocityRatio)x, AVPlayer \(apple.velocityRatio)x
            """
        )
    }

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
            url: try TestServerFixture.asset("test.mp3").absoluteString
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

        // `playingRate` is the accepted target. `currentRate` remains a
        // separate live transport fact and is checked after the media window.
        let requestedRate = 2.0
        player.playingRate = Float(requestedRate)
        try #require(
            abs(Double(player.playingRate) - requestedRate) < 0.05,
            """
            precondition: the player did not accept a playing rate of \
            \(requestedRate); it reports \(player.playingRate)
            """
        )
        let fastMeasurement = try await measureMediaTimeAdvance(player)
        #expect(
            abs(Double(player.currentRate) - requestedRate) < 0.05,
            """
            playingRate reports the accepted target \(player.playingRate), \
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

    private func measureMediaTimeAdvance(
        _ player: AVPlayer
    ) async throws -> MediaTimeMeasurement {
        let clock = ContinuousClock()
        let wallStart = clock.now
        let mediaStart = player.currentTime().seconds
        try await Task.sleep(nanoseconds: 2_000_000_000)
        let mediaAdvance = player.currentTime().seconds - mediaStart
        let elapsed = wallStart.duration(to: clock.now)
        return MediaTimeMeasurement(mediaAdvance: mediaAdvance, elapsed: elapsed)
    }

    private func runKitharaRateScenario(_ fixtureURL: URL) async throws -> RateParityResult {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("rate-parity-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )

        let player = KitharaPlayer(config: .init(store: AssetStore(root: cacheURL.path)))
        let item = KitharaPlayerItem(url: fixtureURL.absoluteString)
        defer {
            player.stop()
            try? FileManager.default.removeItem(at: cacheURL)
        }

        try player.insert(item)
        player.playingRate = 1
        player.play()
        try await waitForRateFact("Kithara playback to advance at live rate 1.0") {
            player.currentTime > 0.1 && abs(player.currentRate - 1) < 0.05
        }

        let normal = try await measureMediaTimeAdvance(player)
        try #require(
            normal.mediaAdvance > 0,
            "precondition: Kithara media time did not advance at rate 1.0"
        )

        let requestedRate: Float = 2
        player.playingRate = requestedRate
        try await waitForRateFact("Kithara live rate to become 2.0") {
            abs(player.currentRate - requestedRate) < 0.05
        }
        let fast = try await measureMediaTimeAdvance(player)
        return RateParityResult(normal: normal, fast: fast)
    }

    private func runAVPlayerRateScenario(_ fixtureURL: URL) async throws -> RateParityResult {
        let player = AVPlayer(playerItem: AVPlayerItem(url: fixtureURL))
        defer {
            player.pause()
            player.replaceCurrentItem(with: nil)
        }

        player.defaultRate = 1
        player.playImmediately(atRate: 1)
        try await waitForRateFact("AVPlayer playback to advance at live rate 1.0") {
            player.currentTime().seconds > 0.1 && abs(player.rate - 1) < 0.05
        }

        let normal = try await measureMediaTimeAdvance(player)
        try #require(
            normal.mediaAdvance > 0,
            "precondition: AVPlayer media time did not advance at rate 1.0"
        )

        let requestedRate: Float = 2
        player.defaultRate = requestedRate
        player.rate = requestedRate
        try await waitForRateFact("AVPlayer live rate to become 2.0") {
            abs(player.rate - requestedRate) < 0.05
        }
        let fast = try await measureMediaTimeAdvance(player)
        return RateParityResult(normal: normal, fast: fast)
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

private struct RateParityResult {
    let normal: MediaTimeMeasurement
    let fast: MediaTimeMeasurement

    var velocityRatio: TimeInterval {
        fast.mediaVelocity / normal.mediaVelocity
    }
}

private struct RateFactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
