import Combine
import Foundation
import Kithara
import Testing

extension LabaIOSTraps {
    @Test("LABA-430 loadedRanges covers a fully downloaded fixture")
    func laba430LoadedRanges() async throws {
        let fixtureURL = try TestServerFixture.asset("test.mp3")
        let bodyLength = try await fixtureContentLength(at: fixtureURL)
        try #require(
            bodyLength > 0,
            "LABA-430 precondition: fixture Content-Length is missing"
        )

        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("laba-430-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )

        let player = KitharaPlayer(
            config: .init(cacheDir: cacheURL.path)
        )
        let item = KitharaPlayerItem(url: fixtureURL.absoluteString)
        let ranges = Laba430Ranges()
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
        try await waitFor430Fact("fixture playback to advance") {
            player.currentTime > 0.1
        }
        try await waitFor430Fact("the fixture duration to settle") {
            (player.duration ?? 0) >= 180
        }
        try await waitFor430Fact("the complete fixture body to reach Kithara's cache") {
            cacheContainsFile(ofSize: bodyLength, under: cacheURL)
        }
        try await waitFor430Fact("loadedRanges to publish a non-empty range") {
            ranges.maximumEnd > 0
        }
        player.pause()

        let duration = try #require(
            player.duration,
            "LABA-430 precondition: duration is unknown after the body downloaded"
        )
        let position = player.currentTime
        try #require(
            position < duration * 0.25,
            """
            LABA-430 precondition: playback had already consumed \(position)s \
            of \(duration)s, leaving no useful downloaded-but-unplayed span
            """
        )

        let loadedEnd = ranges.maximumEnd
        #expect(
            loadedEnd >= duration * 0.9,
            """
            LABA-430: a \(bodyLength)-byte fixture is fully cached, but \
            loadedRanges covers only \(loadedEnd)s of \(duration)s at \
            playback position \(position)s
            """
        )
    }

    private func fixtureContentLength(at url: URL) async throws -> UInt64 {
        var request = URLRequest(url: url)
        request.httpMethod = "HEAD"
        let (_, response) = try await URLSession.shared.data(for: request)
        let http = try #require(
            response as? HTTPURLResponse,
            "LABA-430 precondition: fixture HEAD returned a non-HTTP response"
        )
        try #require(
            (200..<300).contains(http.statusCode),
            "LABA-430 precondition: fixture HEAD returned HTTP \(http.statusCode)"
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

    private func waitFor430Fact(
        _ description: String,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(45))
        while !condition() {
            guard clock.now < deadline else {
                throw Laba430FactTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

private final class Laba430Ranges: @unchecked Sendable {
    private let lock = NSLock()
    private var ranges: [ItemLoadedRange] = []

    var maximumEnd: TimeInterval {
        lock.lock()
        defer { lock.unlock() }
        return ranges.map { $0.start + $0.duration }.max() ?? 0
    }

    func replace(with ranges: [ItemLoadedRange]) {
        lock.lock()
        defer { lock.unlock() }
        self.ranges = ranges
    }
}

private struct Laba430FactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
