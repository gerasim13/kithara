import Combine
import Foundation
import Kithara
import Testing

extension IntegrationRegressionsIOS {
    @Test("Loaded ranges cover a fully downloaded fixture")
    func loadedRangesCoverDownloadedFixture() async throws {
        let fixtureURL = try TestServerFixture.asset("test.mp3")
        let bodyLength = try await fixtureContentLength(at: fixtureURL)
        try #require(
            bodyLength > 0,
            "precondition: fixture Content-Length is missing"
        )

        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("loaded-ranges-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )

        let player = KitharaPlayer(
            config: .init(store: AssetStore(root: cacheURL.path))
        )
        let item = KitharaPlayerItem(url: fixtureURL.absoluteString)
        let ranges = LoadedRangesProbe()
        let cancellable = item.loadedRanges.sink { value in
            ranges.replace(with: value)
        }
        defer {
            player.stop()
            try? FileManager.default.removeItem(at: cacheURL)
            _ = cancellable
        }

        try player.insert(item)
        player.play()
        // Advance first: `item.durationSec` stays 0 until the item's own
        // resource reports one, so the player-side duration is the value that
        // actually settles here.
        try await waitForLoadedRangesFact("fixture playback to advance") {
            player.currentTime > 0.1
        }
        try await waitForLoadedRangesFact("the fixture duration to settle") {
            (player.duration ?? 0) >= 180
        }
        try await waitForLoadedRangesFact("the complete fixture body to reach Kithara's cache") {
            cacheContainsFile(ofSize: bodyLength, under: cacheURL)
        }
        try await waitForLoadedRangesFact("loadedRanges to publish a non-empty range") {
            !ranges.snapshot().isEmpty
        }
        player.pause()

        let duration = try #require(
            player.duration,
            "precondition: duration is unknown after the body downloaded"
        )
        let position = player.currentTime
        try #require(
            position < duration * 0.25,
            """
            precondition: playback had already consumed \(position)s \
            of \(duration)s, leaving no useful downloaded-but-unplayed span
            """
        )

        // `ItemLoadedRange` is `[start, start + duration)`, so a reader that
        // collapses the list to `max(start + duration)` reads the same number
        // whether `duration` is a length or an absolute end, and cannot see a
        // window that begins at the playhead instead of at the origin. Both
        // halves are stated on the published range itself, one per assertion.
        let published = ranges.snapshot()
        let covering = try #require(
            published.min(by: { $0.start < $1.start }),
            "precondition: loadedRanges published no range to inspect"
        )
        #expect(
            covering.start == 0,
            """
            loadedRanges starts at \(covering.start)s rather than at the item's \
            origin, so it reports what is reachable from the playhead at \
            \(position)s instead of what is downloaded
            """
        )
        #expect(
            covering.duration >= duration * 0.9,
            """
            a \(bodyLength)-byte fixture is fully cached, but the range from the \
            origin spans only \(covering.duration)s of \(duration)s at playback \
            position \(position)s
            """
        )
    }

    private func fixtureContentLength(at url: URL) async throws -> UInt64 {
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

    private func cacheContainsFile(ofSize expectedSize: UInt64, under root: URL) -> Bool {
        guard let files = FileManager.default.enumerator(
            at: root,
            includingPropertiesForKeys: [.fileSizeKey, .isRegularFileKey]
        ) else {
            return false
        }

        for case let file as URL in files {
            guard
                let values = try? file.resourceValues(
                    forKeys: [.fileSizeKey, .isRegularFileKey]
                ),
                values.isRegularFile == true,
                let size = values.fileSize
            else {
                continue
            }
            let pathComponents = file.pathComponents
            let isCommittedAsset =
                !file.lastPathComponent.hasSuffix(".tmp")
                && !pathComponents.contains("_index")
            if isCommittedAsset && UInt64(size) == expectedSize {
                return true
            }
        }
        return false
    }

    private func waitForLoadedRangesFact(
        _ description: String,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(45))
        while !condition() {
            guard clock.now < deadline else {
                throw LoadedRangesFactTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

private final class LoadedRangesProbe: @unchecked Sendable {
    private let lock = NSLock()
    private var ranges: [ItemLoadedRange] = []

    func snapshot() -> [ItemLoadedRange] {
        lock.lock()
        defer { lock.unlock() }
        return ranges
    }

    func replace(with ranges: [ItemLoadedRange]) {
        lock.lock()
        defer { lock.unlock() }
        self.ranges = ranges
    }
}

private struct LoadedRangesFactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
