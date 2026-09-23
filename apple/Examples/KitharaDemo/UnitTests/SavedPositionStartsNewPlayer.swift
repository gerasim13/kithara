import AVFAudio
import Foundation
import Kithara
import Testing

extension IntegrationRegressionsIOS {
    @Test("A restored position survives into playback in a new player")
    func savedPositionStartsNewPlayer() async throws {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("saved-position-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )

        let audioSession = AVAudioSession.sharedInstance()
        try audioSession.setCategory(.playback)
        try audioSession.setActive(true)
        defer {
            try? audioSession.setActive(false, options: .notifyOthersOnDeactivation)
            try? FileManager.default.removeItem(at: cacheURL)
        }

        let fixtureURL = try TestServerFixture.signal("signal_mp3_track_sine440_187s.mp3")
            .absoluteString
        let savedPosition = try await captureSavedPosition(
            fixtureURL: fixtureURL,
            cacheURL: cacheURL
        )
        try #require(
            savedPosition >= 4,
            "precondition: the first player reached only \(savedPosition)s"
        )

        try await playFromRestoredPosition(
            fixtureURL: fixtureURL,
            cacheURL: cacheURL,
            savedPosition: savedPosition
        )
    }

    private func captureSavedPosition(
        fixtureURL: String,
        cacheURL: URL
    ) async throws -> TimeInterval {
        let player = KitharaPlayer(
            config: .init(store: AssetStore(root: cacheURL.path))
        )
        let item = KitharaPlayerItem(url: fixtureURL)
        defer {
            player.stop()
        }

        try player.insert(item)
        player.play()
        try await waitForSavedPositionFact("the first player to reach a savable position") {
            player.currentTime >= 4
        }
        player.pause()
        return player.currentTime
    }

    /// Replays the app's relaunch sequence: the queue is seeded, the stored
    /// position is handed to the player before anything has played, and only
    /// then does the user press play. `insert` announces the item as current
    /// right away, which is the moment the app hands its stored position over.
    private func playFromRestoredPosition(
        fixtureURL: String,
        cacheURL: URL,
        savedPosition: TimeInterval
    ) async throws {
        let player = KitharaPlayer(
            config: .init(store: AssetStore(root: cacheURL.path))
        )
        let item = KitharaPlayerItem(url: fixtureURL)
        defer {
            player.stop()
        }

        try player.insert(item)
        let accepted = await withCheckedContinuation { continuation in
            player.seek(to: savedPosition, tolerance: nil) { finished in
                continuation.resume(returning: finished)
            }
        }

        player.play()

        let floor = PlaybackFloor()
        try await waitForSavedPositionFact("playback to advance past the restored position") {
            let position = player.currentTime
            floor.record(position)
            return position >= savedPosition + 2
        }

        let started = try #require(
            floor.value,
            "the replacement player never reported a playing position"
        )
        #expect(
            started >= savedPosition - 1,
            """
            playback ran from \(started)s instead of continuing from \
            \(savedPosition)s; the restore seek reported accepted=\(accepted)
            """
        )
    }

    private func waitForSavedPositionFact(
        _ description: String,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(45))
        while !condition() {
            guard clock.now < deadline else {
                throw SavedPositionTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

/// Lowest position the player ever reported while it was playing. A player
/// that honours the restored position never reports anything below it; one
/// that dropped the position walks up from the head of the track.
private final class PlaybackFloor: @unchecked Sendable {
    private let lock = NSLock()
    private var lowest: TimeInterval?

    var value: TimeInterval? {
        lock.lock()
        defer { lock.unlock() }
        return lowest
    }

    func record(_ position: TimeInterval) {
        guard position > 0 else { return }
        lock.lock()
        defer { lock.unlock() }
        lowest = min(lowest ?? position, position)
    }
}

private struct SavedPositionTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ fact: String) {
        description = "Timed out waiting for \(fact)"
    }
}
