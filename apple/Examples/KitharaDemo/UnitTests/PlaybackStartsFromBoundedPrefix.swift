import Foundation
import Kithara
import Testing

extension IntegrationRegressionsIOS {
    @Test("Playback starts from a bounded prefix, not from the whole file")
    func playbackStartsFromBoundedPrefix() async throws {
        // The server hands over 256 KiB — around sixteen seconds of this
        // fixture — and then holds the connection open forever. Nothing else
        // ever arrives, so every decoded frame has to come out of that prefix.
        // That is what streaming means: the start costs a bounded prefix, and
        // the budget it costs does not grow with the file behind it.
        let prefix = 256 * 1024
        let startupBudget = Duration.seconds(1)
        let patience = Duration.seconds(20)

        let fixture = try await TestServerFixture.registerBehavior(
            .init(
                content: .asset(name: "test.mp3"),
                delivery: .stallAfter(afterBytes: prefix)
            )
        )
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("bounded-prefix-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )

        let player = KitharaPlayer(config: .init(store: AssetStore(root: cacheURL.path)))
        let item = KitharaPlayerItem(
            url: fixture.childURL("test.mp3").absoluteString
        )
        defer {
            player.stop()
            try? FileManager.default.removeItem(at: cacheURL)
        }

        let clock = ContinuousClock()
        // Anchored before the item exists: no byte of the body can have been
        // requested, let alone delivered, before this instant.
        let queued = clock.now
        try player.insert(item)
        player.play()
        try await waitForStartupFact(
            "playback to advance on the \(prefix)-byte prefix the server will ever send",
            within: patience
        ) {
            player.currentTime > 0.1
        }
        let startupLatency = queued.duration(to: clock.now)

        #expect(
            startupLatency < startupBudget,
            """
            audio started \(startupLatency) after the item was queued, on a \
            prefix that was already on the wire; a start budget of \
            \(startupBudget) is what separates streaming from waiting
            """
        )
    }

    private func waitForStartupFact(
        _ description: String,
        within budget: Duration,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: budget)
        while !condition() {
            guard clock.now < deadline else {
                throw StartupFactTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

private struct StartupFactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
