import AVFAudio
import Combine
import Foundation
import Kithara
import Testing

extension IntegrationRegressionsIOS {
    /// An outage that begins before the first sound must not cost the track its
    /// only attempt to start.
    ///
    /// This is the second half of LABA-419 and a different path from
    /// `transient503DoesNotFailCurrentItemAndPlaybackResumes`: nothing is
    /// playing yet, so there is no buffer to starve and no underrun to end. The
    /// track has to begin on its own once connectivity returns, without the user
    /// selecting it again.
    ///
    /// The outage is a refusing server rather than a severed transport, unlike
    /// `seekDuringOutageResumesAfterConnectivityReturns`. What buries the track
    /// here is the load failing at all, and both classes reach the queue the same
    /// way — as a failure the download layer itself reported and gave up on. A
    /// severed transport cannot be used on this path: the playlist body is
    /// drained by its consumer, so the outage surfaces as a hang nobody reports
    /// instead of a failure.
    @Test("An outage before the first sound still starts once connectivity returns")
    func outageBeforeFirstSoundStartsAfterConnectivityReturns() async throws {
        try await withRestoredFixtureNetwork {
            let audioSession = AVAudioSession.sharedInstance()
            try audioSession.setCategory(.playback)
            try audioSession.setActive(true)
            defer {
                try? audioSession.setActive(false, options: .notifyOthersOnDeactivation)
            }

            let cacheURL = FileManager.default.temporaryDirectory
                .appendingPathComponent(
                    "connectivity-first-sound-\(UUID().uuidString)",
                    isDirectory: true
                )
            try FileManager.default.createDirectory(
                at: cacheURL,
                withIntermediateDirectories: true
            )
            defer { try? FileManager.default.removeItem(at: cacheURL) }

            let masterURL = try await TestServerFixture.pacedHlsMasterURL().absoluteString
            let player = KitharaPlayer(config: .init(store: AssetStore(root: cacheURL.path)))
            let item = KitharaPlayerItem(url: masterURL)
            let observation = FirstSoundObservation()
            let itemEvents = item.eventPublisher.sink { observation.record($0) }
            defer {
                itemEvents.cancel()
                player.stop()
            }

            // The network refuses before anything is asked for, so the track never
            // reaches a first byte.
            try await TestServerFixture.setNetwork(.unavailable)

            try player.insert(item)
            player.play()

            try await waitForFirstSoundFact("the outage reaching the player") {
                observation.sawDownloadGiveUp
            }
            try #require(
                player.currentTime == 0,
                "the track produced audio during the outage, so it never needed a recovery"
            )

            try await TestServerFixture.setNetwork(.online)

            try await waitForFirstSoundFact(
                "the track to start on its own after connectivity returned",
                timeout: .seconds(60)
            ) {
                player.currentAudioItem === item && player.currentTime > 1
            }

            #expect(
                player.currentAudioItem === item,
                "recovery replaced the current item"
            )
            #expect(
                player.currentTime > 1,
                "the track never started after connectivity returned"
            )
            #expect(player.currentRate > 0, "the track started but carries no play intent")
        }
    }

    private func withRestoredFixtureNetwork(
        _ operation: () async throws -> Void
    ) async throws {
        try await TestServerFixture.setNetwork(.online)
        do {
            try await operation()
        } catch {
            let operationError = error
            try await TestServerFixture.setNetwork(.online)
            throw operationError
        }
        try await TestServerFixture.setNetwork(.online)
    }

    private func waitForFirstSoundFact(
        _ description: String,
        timeout: Duration = .seconds(75),
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while true {
            try Task.checkCancellation()
            if condition() {
                return
            }
            guard clock.now < deadline else {
                throw FirstSoundFactTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

/// Records that the download layer gave up on a request, which is what proves
/// the outage reached the player rather than being absorbed by a cache.
///
/// Deliberately not keyed on the track being marked failed: that verdict is the
/// defect under test, so a precondition resting on it would evaporate the moment
/// the defect is fixed.
private final class FirstSoundObservation: @unchecked Sendable {
    private let lock = NSLock()
    private var downloadGaveUp = false

    var sawDownloadGiveUp: Bool {
        lock.withLock { downloadGaveUp }
    }

    func record(_ event: ItemEvent) {
        lock.withLock {
            switch event {
            case .downloadRetryExhausted:
                downloadGaveUp = true
            default:
                break
            }
        }
    }
}

private struct FirstSoundFactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
