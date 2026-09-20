import AVFAudio
import Combine
import Foundation
import Kithara
import Testing

extension IntegrationRegressionsIOS {
    @Test("Repeat one completes two full playback cycles")
    func repeatOneCompletesTwoFullPlaybackCycles() async throws {
        let audioSession = AVAudioSession.sharedInstance()
        try audioSession.setCategory(.playback)
        try audioSession.setActive(true)

        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("repeat-mode-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )

        let player = KitharaPlayer(config: .init(store: AssetStore(root: cacheURL.path)))
        let item = KitharaPlayerItem(
            url: try TestServerFixture.signal("signal_wav_silence_1s.wav").absoluteString,
            audioId: 420
        )
        let observation = RepeatTwoCycleObservation(expectedItemID: item.audioId)
        let cancellable = player.eventPublisher.sink { event in
            observation.record(event)
        }
        defer {
            cancellable.cancel()
            player.stop()
            try? audioSession.setActive(false, options: .notifyOthersOnDeactivation)
            try? FileManager.default.removeItem(at: cacheURL)
        }

        player.repeatMode = .one
        try player.insert(item)
        try player.selectItem(item)
        player.play()

        try await waitForRepeatFact(
            "the repeat fixture duration",
            timeout: .seconds(10)
        ) {
            (player.duration ?? 0) >= 0.9
        }
        let duration = try #require(
            player.duration,
            "precondition: the repeat fixture duration is unknown"
        )

        try await waitForRepeatFact(
            "two complete repeat-one playback cycles and the second replay",
            timeout: .seconds(10)
        ) {
            let playback = observation.snapshot()
            return playback.naturalEndCount >= 2 && playback.wrappedAfterSecondEnd
        }

        let playback = observation.snapshot()
        let nearEnd = duration - RepeatTwoCycleObservation.positionTolerance
        #expect(player.repeatMode == .one)
        #expect(playback.observedRepeatOne)
        #expect(
            playback.naturalEndCount >= 2,
            "repeat one emitted fewer than two natural-end events: \(playback.naturalEndCount)"
        )
        #expect(
            playback.firstCycleMaximum >= nearEnd,
            "first cycle ended prematurely: max=\(playback.firstCycleMaximum), duration=\(duration)"
        )
        #expect(
            playback.wrappedAfterFirstEnd,
            "repeat one did not wrap the same item to the beginning after its first natural end"
        )
        #expect(
            playback.secondCycleMaximum >= nearEnd,
            "second cycle ended prematurely: max=\(playback.secondCycleMaximum), duration=\(duration)"
        )
        #expect(playback.wrappedAfterSecondEnd)
        #expect(!playback.sawDifferentItem)
        #expect(!playback.queueEnded)
        #expect(player.currentAudioItem?.audioId == item.audioId)
    }

    private func waitForRepeatFact(
        _ description: String,
        timeout: Duration,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while !condition() {
            guard clock.now < deadline else {
                throw RepeatFactTimeout(description)
            }
            try await Task.sleep(for: .milliseconds(20))
        }
    }
}

private final class RepeatTwoCycleObservation: @unchecked Sendable {
    static let positionTolerance: TimeInterval = 0.25

    private let expectedItemID: TrackId
    private let lock = NSLock()
    private var observedRepeatOne = false
    private var naturalEndCount = 0
    private var firstCycleMaximum: TimeInterval = 0
    private var secondCycleMaximum: TimeInterval = 0
    private var wrappedAfterFirstEnd = false
    private var wrappedAfterSecondEnd = false
    private var sawDifferentItem = false
    private var queueEnded = false

    init(expectedItemID: TrackId) {
        self.expectedItemID = expectedItemID
    }

    func record(_ event: PlayerEvent) {
        lock.lock()
        defer { lock.unlock() }

        switch event {
        case .repeatModeChanged(.one):
            observedRepeatOne = true
        case .timeChanged(let seconds) where seconds.isFinite:
            if naturalEndCount == 0 {
                firstCycleMaximum = max(firstCycleMaximum, seconds)
            } else if naturalEndCount == 1 {
                if !wrappedAfterFirstEnd, seconds <= Self.positionTolerance {
                    wrappedAfterFirstEnd = true
                }
                if wrappedAfterFirstEnd {
                    secondCycleMaximum = max(secondCycleMaximum, seconds)
                }
            } else if naturalEndCount == 2, seconds <= Self.positionTolerance {
                wrappedAfterSecondEnd = true
            }
        case .itemDidPlayToEnd:
            naturalEndCount += 1
        case .currentItemChanged(let itemID):
            if let itemID {
                if itemID != expectedItemID {
                    sawDifferentItem = true
                }
            }
        case .queueEnded:
            queueEnded = true
        default:
            break
        }
    }

    func snapshot() -> RepeatTwoCycleSnapshot {
        lock.lock()
        defer { lock.unlock() }
        return RepeatTwoCycleSnapshot(
            observedRepeatOne: observedRepeatOne,
            naturalEndCount: naturalEndCount,
            firstCycleMaximum: firstCycleMaximum,
            secondCycleMaximum: secondCycleMaximum,
            wrappedAfterFirstEnd: wrappedAfterFirstEnd,
            wrappedAfterSecondEnd: wrappedAfterSecondEnd,
            sawDifferentItem: sawDifferentItem,
            queueEnded: queueEnded
        )
    }
}

private struct RepeatTwoCycleSnapshot {
    let observedRepeatOne: Bool
    let naturalEndCount: Int
    let firstCycleMaximum: TimeInterval
    let secondCycleMaximum: TimeInterval
    let wrappedAfterFirstEnd: Bool
    let wrappedAfterSecondEnd: Bool
    let sawDifferentItem: Bool
    let queueEnded: Bool
}

private struct RepeatFactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
