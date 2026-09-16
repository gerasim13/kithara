import Combine
import Foundation
import Kithara
import Testing

extension IntegrationRegressionsIOS {
    @Test("A transient network failure does not kill the current track")
    func transientFailureKeepsCurrentTrack() async throws {
        try await TestServerFixture.setNetwork(online: true)
        defer {
            Task {
                _ = try? await TestServerFixture.setNetwork(online: true)
            }
        }

        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("transient-failure-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )

        let player = KitharaPlayer(config: .init(store: AssetStore(root: cacheURL.path)))
        let target = KitharaPlayerItem(
            url: try await TestServerFixture.pacedHlsMasterURL().absoluteString
        )
        let fallback = KitharaPlayerItem(
            url: try TestServerFixture.signal("signal_mp3_track_sine440_187s.mp3").absoluteString
        )
        let observation = TransientFailureObservation()
        let currentCancellable = player.currentItem.sink { item in
            observation.recordCurrent(item)
        }
        let eventCancellable = target.eventPublisher.sink { event in
            if case let .downloadFirstByte(_, _, status, _) = event, status == 503 {
                observation.recordUnavailableFetch()
            }
        }
        defer {
            player.stop()
            try? FileManager.default.removeItem(at: cacheURL)
            _ = currentCancellable
            _ = eventCancellable
        }

        try player.insert(target)
        try player.append(fallback)
        try #require(
            player.itemCount == 2,
            "precondition: the target and auto-skip destination were not queued"
        )

        player.play()
        try await waitForTransientFailureFact("the HLS target to become current and advance") {
            observation.matches(target) && player.currentTime > 1
        }
        try await waitForTransientFailureFact("the long HLS duration to settle") {
            (player.duration ?? 0) >= 180
        }

        let failureBeganAt = player.currentTime
        try await TestServerFixture.setNetwork(online: false)
        try await waitForTransientFailureFact("the offline HLS fetch to report HTTP 503") {
            observation.receivedUnavailableFetch
        }
        let positionAtRestore = player.currentTime
        try await TestServerFixture.setNetwork(online: true)

        #expect(
            observation.matches(target),
            """
            the HLS target received HTTP 503 after the outage began at \
            \(failureBeganAt)s, but the queue moved to item \
            \(observation.currentID.map(String.init) ?? "nil"); \
            fallback=\(fallback.id)
            """
        )
        guard observation.matches(target) else {
            return
        }

        let recoveryTarget = positionAtRestore + 5
        let recovered = await reachedTransientFailureFact(deadline: .seconds(45)) {
            observation.matches(target)
                && player.currentTime >= recoveryTarget
        }
        #expect(
            recovered,
            """
            the HLS target remained selected after the transient \
            failure but never advanced from \(positionAtRestore)s to \
            \(recoveryTarget)s after its HTTP 503 response
            """
        )
        #expect(
            observation.matches(target),
            """
            the queue auto-skipped to \
            \(observation.currentID.map(String.init) ?? "nil") during recovery
            """
        )
    }

    private func reachedTransientFailureFact(
        deadline duration: Duration,
        condition: () -> Bool
    ) async -> Bool {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: duration)
        while clock.now < deadline {
            if condition() {
                return true
            }
            try? await Task.sleep(nanoseconds: 20_000_000)
        }
        return condition()
    }

    private func waitForTransientFailureFact(
        _ description: String,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(45))
        while !condition() {
            guard clock.now < deadline else {
                throw TransientFailureTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

private final class TransientFailureObservation: @unchecked Sendable {
    private let lock = NSLock()
    private var itemID: Int64?
    private var unavailableFetch = false

    var currentID: Int64? {
        lock.lock()
        defer { lock.unlock() }
        return itemID
    }

    var receivedUnavailableFetch: Bool {
        lock.lock()
        defer { lock.unlock() }
        return unavailableFetch
    }

    func recordCurrent(_ item: KitharaPlayerItem?) {
        lock.lock()
        defer { lock.unlock() }
        itemID = item?.id
    }

    func recordUnavailableFetch() {
        lock.lock()
        defer { lock.unlock() }
        unavailableFetch = true
    }

    func matches(_ item: KitharaPlayerItem) -> Bool {
        currentID == item.id
    }
}

private struct TransientFailureTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
