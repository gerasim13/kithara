import Foundation
import Kithara
import Testing

extension LabaIOSTraps {
    @Test("LABA-418 applies a playing-rate change during playback")
    func laba418RateAppliesWhilePlaying() async throws {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("laba-418-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )

        let player = KitharaPlayer(config: .init(cacheDir: cacheURL.path))
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
        try await waitFor418Fact("fixture playback to advance at rate 1.0") {
            player.currentTime > 0.1
                && abs(player.currentRate - 1) < 0.05
        }

        let normalAdvance = try await measure418Advance(player)
        try #require(
            normalAdvance > 0,
            "LABA-418 precondition: media time did not advance during the rate-1.0 window"
        )

        // Deliberately not waiting on `currentRate` here: the live rate and the
        // target rate are separate values, and blocking on the live one would
        // turn this trap into a timeout instead of a measurement of the symptom
        // QA reported — playback speed that ignores the requested rate.
        player.playingRate = 2
        try #require(
            abs(player.playingRate - 2) < 0.05,
            """
            LABA-418 precondition: the player did not accept a playing rate of \
            2.0; it reports \(player.playingRate)
            """
        )
        let fastAdvance = try await measure418Advance(player)

        #expect(
            fastAdvance >= normalAdvance * 1.5,
            """
            LABA-418: changing playingRate from 1.0 to 2.0 while playing \
            advanced media time by only \(fastAdvance)s versus \
            \(normalAdvance)s over equal wall-clock windows
            """
        )
    }

    private func measure418Advance(_ player: KitharaPlayer) async throws -> TimeInterval {
        let start = player.currentTime
        try await Task.sleep(nanoseconds: 2_000_000_000)
        return player.currentTime - start
    }

    private func waitFor418Fact(
        _ description: String,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(30))
        while !condition() {
            guard clock.now < deadline else {
                throw Laba418FactTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

private struct Laba418FactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
