import Foundation
import Kithara
import Testing

extension IntegrationRegressionsIOS {
    @Test("Playback starts before a slowly delivered body has finished arriving")
    func playbackStartsBeforeBodyFinishesDownloading() async throws {
        // The fixture arrives in fixed-size pieces with a fixed pause between
        // them, so the earliest instant its last byte can land is a property of
        // the server, not of how fast this machine decodes. A start that beats
        // that instant provably did not wait for the whole file.
        let chunk = 16 * 1024
        let delayMilliseconds: UInt64 = 100
        let bodyLength = try await startupBodyLength(at: TestServerFixture.asset("test.mp3"))
        try #require(
            bodyLength > 0,
            "precondition: fixture Content-Length is missing"
        )
        let pieces = (bodyLength + UInt64(chunk) - 1) / UInt64(chunk)
        let fullBodyArrival = Duration.milliseconds(pieces * delayMilliseconds)

        let fixture = try await TestServerFixture.registerBehavior(
            .init(
                content: .asset(name: "test.mp3"),
                delivery: .throttle(
                    chunk: chunk,
                    delayMilliseconds: delayMilliseconds
                )
            )
        )
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("startup-before-download-\(UUID().uuidString)", isDirectory: true)
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
            "throttled playback to advance",
            within: fullBodyArrival * 3
        ) {
            player.currentTime > 0.1
        }
        let startupLatency = queued.duration(to: clock.now)

        #expect(
            startupLatency < fullBodyArrival,
            """
            audio started \(startupLatency) after the item was queued, while \
            the server needs at least \(fullBodyArrival) to hand over all \
            \(bodyLength) bytes in \(chunk)-byte pieces; the start waited for \
            the download rather than for the first frames
            """
        )
    }

    private func startupBodyLength(at url: URL) async throws -> UInt64 {
        var request = URLRequest(url: url)
        request.httpMethod = "HEAD"
        let (_, response) = try await URLSession.shared.data(for: request)
        let http = try #require(
            response as? HTTPURLResponse,
            "precondition: fixture HEAD returned a non-HTTP response"
        )
        try #require(
            (200..<300).contains(http.statusCode),
            "precondition: fixture HEAD returned HTTP \(http.statusCode)"
        )
        return UInt64(max(0, http.expectedContentLength))
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
