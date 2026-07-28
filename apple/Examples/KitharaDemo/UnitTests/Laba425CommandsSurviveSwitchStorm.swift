import Combine
import Foundation
import Kithara
import Testing

extension LabaIOSTraps {
    @Test("LABA-425 keeps commands alive after a track-switch storm")
    func laba425CommandsSurviveSwitchStorm() async throws {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("laba-425-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )

        let fixture = try await throttled425Fixture()
        let items = (0..<3).map { index in
            KitharaPlayerItem(
                url: fixture.childURL("storm-\(index).mp3").absoluteString
            )
        }
        let player = KitharaPlayer(config: .init(cacheDir: cacheURL.path))
        let current = Laba425CurrentItem()
        let currentCancellable = player.currentItem.sink { item in
            current.record(item)
        }
        defer {
            player.stop()
            try? FileManager.default.removeItem(at: cacheURL)
            _ = currentCancellable
        }

        try player.insert(items[0])
        try player.append(items[1])
        try player.append(items[2])
        try #require(
            player.itemCount == items.count,
            "LABA-425 precondition: the three-item queue was not constructed"
        )

        player.play()
        try await waitFor425Fact("the first throttled track to start") {
            current.matches(items[0]) && player.currentTime > 0.1
        }

        let targets = [
            items[1], items[2], items[0],
            items[2], items[1], items[0],
            items[1], items[2], items[0],
            items[2], items[1], items[0],
        ]
        for (round, target) in targets.enumerated() {
            do {
                try player.selectItem(target, transition: .none)
            } catch {
                throw Laba425CommandFailure(
                    "switch \(round) to item \(target.id) was rejected: \(error)"
                )
            }
            let switched = await reached425Fact(deadline: .seconds(30)) {
                current.matches(target)
            }
            #expect(
                switched,
                """
                LABA-425: switch \(round) never made item \(target.id) current; \
                observed=\(current.itemID.map(String.init) ?? "nil")
                """
            )
            guard switched else {
                return
            }
        }

        try await waitFor425Fact("the final switched-to track to advance") {
            player.currentTime > 0.1
        }
        try await waitFor425Fact("the final track duration to settle") {
            (player.duration ?? 0) >= 180
        }

        let seekTarget: TimeInterval = 5
        try #require(
            player.currentTime < seekTarget - 1,
            """
            LABA-425 precondition: the final track had already reached \
            \(player.currentTime)s before the \(seekTarget)s seek
            """
        )
        let accepted = await withCheckedContinuation { continuation in
            player.seek(to: seekTarget, tolerance: nil) { finished in
                continuation.resume(returning: finished)
            }
        }
        #expect(
            accepted,
            "LABA-425: seek was rejected after the track-switch storm"
        )
        guard accepted else {
            return
        }

        let landed = await reached425Fact(deadline: .seconds(30)) {
            abs(player.currentTime - seekTarget) < 1
        }
        #expect(
            landed,
            """
            LABA-425: seek to \(seekTarget)s did not land after the switch \
            storm; current=\(player.currentTime)s
            """
        )
        guard landed else {
            return
        }

        player.pause()
        let paused = await reached425Fact(deadline: .seconds(30)) {
            player.currentRate == 0
        }
        #expect(
            paused,
            """
            LABA-425: pause was swallowed after the switch storm; \
            rate=\(player.currentRate)
            """
        )
        guard paused else {
            return
        }

        let pausedAt = player.currentTime
        player.play()
        let playing = await reached425Fact(deadline: .seconds(30)) {
            player.currentRate > 0
        }
        #expect(
            playing,
            """
            LABA-425: play was swallowed after the switch storm; \
            rate=\(player.currentRate)
            """
        )
        guard playing else {
            return
        }

        let carriedOn = await reached425Fact(deadline: .seconds(45)) {
            player.currentTime >= pausedAt + 1
        }
        #expect(
            carriedOn,
            """
            LABA-425: playback reported a positive rate after the storm but \
            media time never advanced past \(pausedAt)s
            """
        )
    }

    /// Throttled so the switch storm lands while transfers are still in
    /// flight, which is the state the report describes. The fixture is named
    /// rather than uploaded — a 3 MB body exceeds the server's request limit.
    private func throttled425Fixture() async throws -> TestServerFixture.BehaviorHandle {
        try await TestServerFixture.registerBehavior(
            .init(
                content: .asset(name: "test.mp3"),
                delivery: .throttle(chunk: 4 * 1024, delayMilliseconds: 20)
            )
        )
    }

    private func reached425Fact(
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

    private func waitFor425Fact(
        _ description: String,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(60))
        while !condition() {
            guard clock.now < deadline else {
                throw Laba425FactTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

private final class Laba425CurrentItem: @unchecked Sendable {
    private let lock = NSLock()
    private var currentID: Int64?

    var itemID: Int64? {
        lock.lock()
        defer { lock.unlock() }
        return currentID
    }

    func record(_ item: KitharaPlayerItem?) {
        lock.lock()
        defer { lock.unlock() }
        currentID = item?.id
    }

    func matches(_ item: KitharaPlayerItem) -> Bool {
        itemID == item.id
    }
}

private struct Laba425CommandFailure: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "LABA-425: \(description)"
    }
}

private struct Laba425FactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
