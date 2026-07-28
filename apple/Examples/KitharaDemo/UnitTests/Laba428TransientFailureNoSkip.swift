import Combine
import Foundation
import Kithara
import Testing

extension LabaIOSTraps {
    @Test("LABA-428 keeps the current track after a transient network failure")
    func laba428TransientFailureNoSkip() async throws {
        try await TestServerFixture.setNetwork(online: true)
        defer {
            Task {
                _ = try? await TestServerFixture.setNetwork(online: true)
            }
        }

        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("laba-428-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )

        let player = KitharaPlayer(config: .init(cacheDir: cacheURL.path))
        let target = KitharaPlayerItem(
            url: try TestServerFixture.asset("hls/master.m3u8").absoluteString
        )
        let fallback = KitharaPlayerItem(
            url: try TestServerFixture.asset("test.mp3").absoluteString
        )
        let observation = Laba428Observation()
        let currentCancellable = player.currentItem.sink { item in
            observation.recordCurrent(item)
        }
        let stallCancellable = target.didStall.sink {
            observation.recordStall()
        }
        let errorCancellable = target.error.sink { error in
            observation.recordFailure(error)
        }
        defer {
            player.stop()
            try? FileManager.default.removeItem(at: cacheURL)
            _ = currentCancellable
            _ = stallCancellable
            _ = errorCancellable
        }

        try player.insert(target)
        try player.append(fallback)
        try #require(
            player.itemCount == 2,
            "LABA-428 precondition: the target and auto-skip destination were not queued"
        )

        player.play()
        try await waitFor428Fact("the HLS target to become current and advance") {
            observation.matches(target) && player.currentTime > 1
        }
        try await waitFor428Fact("the long HLS duration to settle") {
            (player.duration ?? 0) >= 180
        }

        let failureBeganAt = player.currentTime
        try await TestServerFixture.setNetwork(online: false)
        let failureFact = await observe428Failure(
            player,
            observation: observation,
            fallback: fallback
        )
        let positionAtRestore = player.currentTime
        try await TestServerFixture.setNetwork(online: true)

        try #require(
            failureFact != nil,
            """
            LABA-428 precondition: the brief outage produced no observable \
            stall, failure, skip, or stopped playback; it began at \
            \(failureBeganAt)s
            """
        )
        #expect(
            observation.matches(target),
            """
            LABA-428: the transient failure permanently failed the HLS target \
            and moved the queue to item \
            \(observation.currentID.map(String.init) ?? "nil"); \
            fallback=\(fallback.id), signal=\(failureFact?.description ?? "none")
            """
        )
        guard observation.matches(target) else {
            return
        }

        let recoveryTarget = positionAtRestore + 5
        let recovered = await reached428Fact(deadline: .seconds(45)) {
            observation.matches(target)
                && player.currentTime >= recoveryTarget
        }
        #expect(
            recovered,
            """
            LABA-428: the HLS target remained selected after the transient \
            failure but never advanced from \(positionAtRestore)s to \
            \(recoveryTarget)s; signal=\(failureFact?.description ?? "none")
            """
        )
        #expect(
            observation.matches(target),
            """
            LABA-428: the queue auto-skipped to \
            \(observation.currentID.map(String.init) ?? "nil") during recovery
            """
        )
    }

    private func observe428Failure(
        _ player: KitharaPlayer,
        observation: Laba428Observation,
        fallback: KitharaPlayerItem
    ) async -> Laba428FailureFact? {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(45))
        var lastPosition = player.currentTime
        var stableSince = clock.now

        while clock.now < deadline {
            if observation.matches(fallback) {
                return .autoSkipped
            }
            if let failure = observation.failure {
                return .failed(failure)
            }
            if observation.didStall {
                return .stalled
            }

            let position = player.currentTime
            if position > lastPosition + 0.02 {
                lastPosition = position
                stableSince = clock.now
            } else if stableSince.duration(to: clock.now) >= .seconds(2) {
                return .stoppedAdvancing(position)
            }
            try? await Task.sleep(nanoseconds: 20_000_000)
        }
        return nil
    }

    private func reached428Fact(
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

    private func waitFor428Fact(
        _ description: String,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(45))
        while !condition() {
            guard clock.now < deadline else {
                throw Laba428FactTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

private enum Laba428FailureFact: CustomStringConvertible {
    case autoSkipped
    case failed(String)
    case stalled
    case stoppedAdvancing(TimeInterval)

    var description: String {
        switch self {
        case .autoSkipped:
            "auto-skipped"
        case .failed(let error):
            "failed: \(error)"
        case .stalled:
            "didStall"
        case .stoppedAdvancing(let position):
            "stopped at \(position)s"
        }
    }
}

private final class Laba428Observation: @unchecked Sendable {
    private let lock = NSLock()
    private var itemID: Int64?
    private var stalled = false
    private var failureDescription: String?

    var currentID: Int64? {
        lock.lock()
        defer { lock.unlock() }
        return itemID
    }

    var didStall: Bool {
        lock.lock()
        defer { lock.unlock() }
        return stalled
    }

    var failure: String? {
        lock.lock()
        defer { lock.unlock() }
        return failureDescription
    }

    func recordCurrent(_ item: KitharaPlayerItem?) {
        lock.lock()
        defer { lock.unlock() }
        itemID = item?.id
    }

    func recordStall() {
        lock.lock()
        defer { lock.unlock() }
        stalled = true
    }

    func recordFailure(_ error: Error) {
        lock.lock()
        defer { lock.unlock() }
        failureDescription = String(describing: error)
    }

    func matches(_ item: KitharaPlayerItem) -> Bool {
        currentID == item.id
    }
}

private struct Laba428FactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
