import AVFAudio
import Combine
import Foundation
import Kithara
import Testing

extension IntegrationRegressionsIOS {
    @Test("Successive public next commands play every queued item")
    func successivePublicNextCommandsPlayEveryQueuedItem() async throws {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("public-next-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )

        let items = try await publicNextItems()
        try #require(
            Set(items.map(\.id)).count == items.count
                && Set(items.map(\.url)).count == items.count,
            "precondition: public-next fixtures are not unique"
        )

        let player = KitharaPlayer(config: .init(store: AssetStore(root: cacheURL.path)))
        let observation = PublicNextObservation()
        let audioSession = AVAudioSession.sharedInstance()
        try audioSession.setCategory(.playback)
        try audioSession.setActive(true)
        defer {
            try? audioSession.setActive(false, options: .notifyOthersOnDeactivation)
        }
        let eventCancellable = player.eventPublisher.sink { event in
            observation.record(event: event)
        }
        let errorCancellable = player.error.sink { error in
            observation.record(error: error)
        }
        defer {
            player.stop()
            try? FileManager.default.removeItem(at: cacheURL)
            _ = eventCancellable
            _ = errorCancellable
        }

        let first = items[0]
        try player.insert(first)
        let beforePlay = await first.load()
        try #require(
            !beforePlay.isPlayable,
            "precondition: the first item became playable before play started during loading"
        )
        player.play()
        try await waitForPublicNextPlayback(
            of: first,
            player: player,
            observation: observation
        )

        for (index, target) in items.dropFirst().enumerated() {
            try player.append(target)
            let load = await target.load()
            try #require(
                !load.isPlayable,
                "precondition: item \(index + 1) became playable before public next"
            )
            player.advanceToNextItem()
            try await waitForPublicNextPlayback(
                of: target,
                player: player,
                observation: observation
            )
        }

        try player.selectItem(first, transition: .none)
        try await waitForPublicNextPlayback(
            of: first,
            player: player,
            observation: observation
        )
    }

    private func publicNextItems() async throws -> [KitharaPlayerItem] {
        let deliveries: [(chunk: Int, delayMilliseconds: UInt64)] = [
            (16 * 1024, 22),
            (8 * 1024, 20),
            (4 * 1024, 20),
        ]
        var items: [KitharaPlayerItem] = []
        for (index, delivery) in deliveries.enumerated() {
            let fixture = try await TestServerFixture.registerBehavior(
                .init(
                    content: .signal(name: "signal_mp3_track_sine440_187s.mp3"),
                    delivery: .throttle(
                        chunk: delivery.chunk,
                        delayMilliseconds: delivery.delayMilliseconds
                    )
                )
            )
            let itemID = 42_500 + index
            items.append(
                KitharaPlayerItem(
                    url: fixture.childURL("public-next-\(index).mp3").absoluteString,
                    audioId: itemID,
                    uuid: Int64(itemID)
                )
            )
        }
        return items
    }

    private func waitForPublicNextPlayback(
        of item: KitharaPlayerItem,
        player: KitharaPlayer,
        observation: PublicNextObservation
    ) async throws {
        try await waitForPublicNextFact(
            "item \(item.audioId) to become current with a fresh clock",
            observation: observation
        ) {
            player.currentAudioItem?.id == item.id && player.currentTime < 1
        }
        let baseline = player.currentTime
        try await waitForPublicNextFact(
            "item \(item.audioId) media time to advance",
            observation: observation
        ) {
            player.currentAudioItem?.id == item.id
                && player.currentTime >= baseline + 0.15
        }
    }

    private func waitForPublicNextFact(
        _ description: String,
        observation: PublicNextObservation,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(60))
        while true {
            if let failure = observation.failure {
                throw PublicNextFailure(failure)
            }
            if condition() {
                return
            }
            guard clock.now < deadline else {
                throw PublicNextFailure("Timed out waiting for \(description)")
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

private final class PublicNextObservation: @unchecked Sendable {
    private let lock = NSLock()
    private var failures: [String] = []

    var failure: String? {
        lock.lock()
        defer { lock.unlock() }
        return failures.first
    }

    func record(event: PlayerEvent) {
        lock.lock()
        defer { lock.unlock() }
        switch event {
        case let .trackStatusChanged(itemId, status):
            if case let .failed(reason) = status {
                failures.append("item \(itemId) failed: \(reason)")
            }
        case let .itemDidFail(itemId):
            failures.append("item \(itemId.map(String.init) ?? "unknown") failed")
        case let .error(message):
            failures.append(message)
        default:
            break
        }
    }

    func record(error: Error) {
        lock.lock()
        defer { lock.unlock() }
        failures.append(String(describing: error))
    }
}

private struct PublicNextFailure: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = description
    }
}
