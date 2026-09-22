import AVFAudio
import Combine
import Foundation
import Kithara
import Testing

extension IntegrationRegressionsIOS {
    /// A seek taken while the network is down must not strand playback once
    /// connectivity returns.
    ///
    /// This is the scenario `transient503DoesNotFailCurrentItemAndPlaybackResumes`
    /// does not reach: it never seeks, and it takes the network down as a
    /// reachable server answering `503`. On a device the outage is a transport
    /// failure and the user does seek — which is what leaves the track sitting at
    /// a position whose bytes were written off while the radio was away.
    ///
    /// Seeking past the drained buffer guarantees the target is not already
    /// cached, so recovery has to fetch for it rather than replay.
    @Test("A seek during an outage still resumes once connectivity returns")
    func seekDuringOutageResumesAfterConnectivityReturns() async throws {
        try await withRestoredFixtureNetwork {
            let audioSession = AVAudioSession.sharedInstance()
            try audioSession.setCategory(.playback)
            try audioSession.setActive(true)
            defer {
                try? audioSession.setActive(false, options: .notifyOthersOnDeactivation)
            }

            let cacheURL = FileManager.default.temporaryDirectory
                .appendingPathComponent(
                    "connectivity-seek-\(UUID().uuidString)",
                    isDirectory: true
                )
            try FileManager.default.createDirectory(
                at: cacheURL,
                withIntermediateDirectories: true
            )
            defer { try? FileManager.default.removeItem(at: cacheURL) }

            let player = KitharaPlayer(config: .init(store: AssetStore(root: cacheURL.path)))
            let item = KitharaPlayerItem(
                url: try await TestServerFixture.pacedHlsMasterURL().absoluteString
            )
            let observation = SeekDuringOutageObservation()
            let itemEvents = item.eventPublisher.sink { observation.record($0) }
            defer {
                itemEvents.cancel()
                player.stop()
            }

            try player.insert(item)
            player.play()
            try await waitForSeekOutageFact("paced HLS playback before the outage") {
                player.currentAudioItem === item
                    && player.currentRate > 0
                    && player.currentTime > 1
                    && (player.duration ?? 0) >= 180
            }

            try await TestServerFixture.setNetwork(.transportFailure)
            try await waitForSeekOutageFact("the download layer giving up during the outage") {
                observation.sawDownloadGiveUp
            }
            // Deliberately not waiting for an underrun as well. A buried segment
            // ends the read rather than starving it, so on broken code the buffer
            // never drains and a precondition resting on that reports a missing
            // underrun instead of the defect — the trap would go red for the wrong
            // reason. Giving up on a request already proves the outage arrived.
            //
            // Far past the reader, so the target's bytes are ones the outage was
            // in a position to write off rather than anything already cached.
            let seekTarget = player.currentTime + 30
            // Acceptance is reported synchronously from the queue's own verdict,
            // not when the seek lands, so this resumes even with the radio away.
            let accepted = await withCheckedContinuation { continuation in
                player.seek(to: seekTarget, tolerance: nil) { finished in
                    continuation.resume(returning: finished)
                }
            }
            try #require(accepted, "the seek was rejected while the network was down")

            try await TestServerFixture.setNetwork(.online)

            let resumeTarget = seekTarget + 1
            try await waitForSeekOutageFact(
                "the track to carry on from the seek target after connectivity returned",
                timeout: .seconds(60)
            ) {
                player.currentAudioItem === item && player.currentTime >= resumeTarget
            }

            #expect(player.currentAudioItem === item, "recovery replaced the current item")
            #expect(
                player.currentTime >= resumeTarget,
                "the track never advanced past the seek target after connectivity returned"
            )
            #expect(player.currentRate > 0, "recovery lost play intent")
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

    private func waitForSeekOutageFact(
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
                throw SeekOutageFactTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

/// The fact that establishes the outage actually reached playback: the download
/// layer spent a request's budget and gave up on it.
///
/// Not keyed on the track being marked failed — that verdict is the defect under
/// test, and a precondition resting on it would evaporate with the fix.
private final class SeekDuringOutageObservation: @unchecked Sendable {
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

private struct SeekOutageFactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
