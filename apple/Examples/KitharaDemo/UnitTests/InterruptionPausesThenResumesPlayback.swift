import AVFoundation
import Combine
import Foundation
import Kithara
import Testing

@Suite("Integration regressions (iOS)", .serialized)
struct IntegrationRegressionsIOS {}

extension IntegrationRegressionsIOS {
    @Test("An audio-session interruption leaves playback controls responsive")
    func interruptionPausesThenResumesPlayback() async throws {
        let audioSession = AVAudioSession.sharedInstance()
        try audioSession.setCategory(.playback)
        try audioSession.setActive(true)

        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("interruption-resume-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )
        let player = KitharaPlayer(config: .init(store: AssetStore(root: cacheURL.path)))
        let item = KitharaPlayerItem(
            url: try TestServerFixture.asset("test.mp3").absoluteString
        )
        let rates = InterruptionRates()
        let cancellable = player.rate.sink { rate in
            rates.record(rate)
        }
        defer {
            player.stop()
            try? audioSession.setActive(false, options: .notifyOthersOnDeactivation)
            try? FileManager.default.removeItem(at: cacheURL)
            _ = cancellable
        }

        try player.insert(item)
        player.play()
        try await waitForInterruptionFact("fixture playback to advance") {
            player.currentTime > 0.1
        }
        try #require(
            player.currentRate > 0,
            "precondition: fixture playback did not start"
        )

        // The player reacts across the FFI boundary, so reading the rate in the
        // same breath as posting the notification would race a correct
        // implementation and keep this trap red even once it is fixed.
        rates.reset()
        postInterruption(.began)
        let paused = await reachedInterruptionFact { player.currentRate == 0 }
        #expect(
            paused,
            """
            playback did not pause after AVAudioSession posted \
            interruption-began; rate=\(player.currentRate), \
            published=\(rates.snapshot())
            """
        )

        rates.reset()
        postInterruption(.ended, options: .shouldResume)
        let resumed = await reachedInterruptionFact { player.currentRate > 0 }
        #expect(
            resumed,
            """
            playback did not resume after AVAudioSession ended the \
            interruption with .shouldResume; rate=\(player.currentRate), \
            published=\(rates.snapshot())
            """
        )

        rates.reset()
        postInterruption(.began)
        let pausedWithoutAutomaticResume = await reachedInterruptionFact {
            player.currentRate == 0
        }
        #expect(
            pausedWithoutAutomaticResume,
            """
            playback did not pause before the no-resume interruption ended; \
            rate=\(player.currentRate), published=\(rates.snapshot())
            """
        )

        rates.reset()
        postInterruption(.ended)
        #expect(
            player.currentRate == 0,
            """
            playback resumed after AVAudioSession ended the interruption \
            without .shouldResume; rate=\(player.currentRate), \
            published=\(rates.snapshot())
            """
        )

        player.play()
        let resumedByControl = await reachedInterruptionFact {
            player.currentRate > 0
        }
        #expect(
            resumedByControl,
            """
            play() did not resume playback after the no-resume interruption; \
            rate=\(player.currentRate), published=\(rates.snapshot())
            """
        )

        rates.reset()
        player.pause()
        let pausedByControl = await reachedInterruptionFact {
            player.currentRate == 0
        }
        #expect(
            pausedByControl,
            """
            pause() did not pause playback after the no-resume interruption; \
            rate=\(player.currentRate), published=\(rates.snapshot())
            """
        )
    }

    @Test("Interruption control flow matches AVQueuePlayer")
    func interruptionControlFlowMatchesAVQueuePlayer() async throws {
        let audioSession = AVAudioSession.sharedInstance()
        try audioSession.setCategory(.playback)
        try audioSession.setActive(true)
        defer {
            try? audioSession.setActive(false, options: .notifyOthersOnDeactivation)
        }

        let fixtureURL = try TestServerFixture.asset("test.mp3")
        let kithara = try await observeKitharaInterruptionControlFlow(url: fixtureURL)
        let apple = try await observeAVQueuePlayerInterruptionControlFlow(url: fixtureURL)
        let expectedTrace = [
            InterruptionPhaseState(phase: .started, isPlaying: true),
            InterruptionPhaseState(phase: .interruptionBegan, isPlaying: false),
            InterruptionPhaseState(phase: .endedShouldResume, isPlaying: true),
            InterruptionPhaseState(phase: .secondInterruptionBegan, isPlaying: false),
            InterruptionPhaseState(phase: .endedWithoutResume, isPlaying: false),
            InterruptionPhaseState(phase: .resumedByControl, isPlaying: true),
            InterruptionPhaseState(phase: .pausedByControl, isPlaying: false),
        ]

        #expect(
            kithara.trace == expectedTrace,
            "Kithara interruption trace was not expected-good: \(kithara.trace)"
        )
        #expect(
            apple.trace == expectedTrace,
            "AVQueuePlayer interruption trace was not expected-good: \(apple.trace)"
        )
        #expect(
            kithara.trace == apple.trace,
            "phase traces differ: Kithara=\(kithara.trace), AVQueuePlayer=\(apple.trace)"
        )
        #expect(
            kithara.resumeDisplacement >= 0.15,
            "Kithara did not advance after shouldResume: \(kithara.resumeDisplacement)"
        )
        #expect(
            apple.resumeDisplacement >= 0.15,
            "AVQueuePlayer did not advance after shouldResume: \(apple.resumeDisplacement)"
        )
        #expect(
            abs(kithara.resumeDisplacement - apple.resumeDisplacement) <= 0.5,
            """
            resume displacement differs: Kithara=\(kithara.resumeDisplacement), \
            AVQueuePlayer=\(apple.resumeDisplacement)
            """
        )
    }

    private func observeKitharaInterruptionControlFlow(
        url: URL
    ) async throws -> InterruptionObservation {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("interruption-parity-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )
        let player = KitharaPlayer(config: .init(store: AssetStore(root: cacheURL.path)))
        let item = KitharaPlayerItem(url: url.absoluteString)
        defer {
            player.stop()
            try? FileManager.default.removeItem(at: cacheURL)
        }

        try player.insert(item)
        return try await observeInterruptionControlFlow(
            InterruptionControls(
                play: player.play,
                pause: player.pause,
                isPlaying: { player.currentRate > 0 },
                position: { player.currentTime }
            ),
            backend: "Kithara"
        )
    }

    private func observeAVQueuePlayerInterruptionControlFlow(
        url: URL
    ) async throws -> InterruptionObservation {
        let player = AVQueuePlayer(items: [AVPlayerItem(url: url)])
        let adapter = AVQueuePlayerInterruptionAdapter(player: player)
        defer {
            player.pause()
            player.removeAllItems()
            _ = adapter
        }

        return try await observeInterruptionControlFlow(
            InterruptionControls(
                play: player.play,
                pause: player.pause,
                isPlaying: { player.rate > 0 },
                position: {
                    let seconds = CMTimeGetSeconds(player.currentTime())
                    return seconds.isFinite ? max(0, seconds) : 0
                }
            ),
            backend: "AVQueuePlayer"
        )
    }

    private func observeInterruptionControlFlow(
        _ controls: InterruptionControls,
        backend: String
    ) async throws -> InterruptionObservation {
        var trace: [InterruptionPhaseState] = []
        func record(_ phase: InterruptionPhase) {
            trace.append(InterruptionPhaseState(phase: phase, isPlaying: controls.isPlaying()))
        }

        controls.play()
        try await waitForInterruptionFact("\(backend) fixture playback to advance") {
            controls.isPlaying() && controls.position() > 0.1
        }
        record(.started)

        postInterruption(.began)
        _ = await reachedInterruptionFact { !controls.isPlaying() }
        record(.interruptionBegan)

        let resumeOrigin = controls.position()
        postInterruption(.ended, options: .shouldResume)
        _ = await reachedInterruptionFact { controls.isPlaying() }
        record(.endedShouldResume)
        _ = await reachedInterruptionFact {
            controls.position() - resumeOrigin >= 0.2
        }
        let resumeDisplacement = max(0, controls.position() - resumeOrigin)

        postInterruption(.began)
        _ = await reachedInterruptionFact { !controls.isPlaying() }
        record(.secondInterruptionBegan)

        postInterruption(.ended)
        record(.endedWithoutResume)

        controls.play()
        _ = await reachedInterruptionFact { controls.isPlaying() }
        record(.resumedByControl)

        controls.pause()
        _ = await reachedInterruptionFact { !controls.isPlaying() }
        record(.pausedByControl)

        return InterruptionObservation(
            trace: trace,
            resumeDisplacement: resumeDisplacement
        )
    }

    private func postInterruption(
        _ type: AVAudioSession.InterruptionType,
        options: AVAudioSession.InterruptionOptions = []
    ) {
        var userInfo: [AnyHashable: Any] = [
            AVAudioSessionInterruptionTypeKey: type.rawValue
        ]
        if type == .ended {
            userInfo[AVAudioSessionInterruptionOptionKey] = options.rawValue
        }
        NotificationCenter.default.post(
            name: AVAudioSession.interruptionNotification,
            object: AVAudioSession.sharedInstance(),
            userInfo: userInfo
        )
    }

    /// Bounded poll returning whether the fact arrived, so a missing reaction
    /// fails the expectation instead of throwing out of the test.
    private func reachedInterruptionFact(_ condition: () -> Bool) async -> Bool {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(5))
        while clock.now < deadline {
            if condition() {
                return true
            }
            try? await Task.sleep(nanoseconds: 20_000_000)
        }
        return condition()
    }

    private func waitForInterruptionFact(
        _ description: String,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(30))
        while !condition() {
            guard clock.now < deadline else {
                throw InterruptionFactTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

private enum InterruptionPhase: String, Equatable {
    case started
    case interruptionBegan
    case endedShouldResume
    case secondInterruptionBegan
    case endedWithoutResume
    case resumedByControl
    case pausedByControl
}

private struct InterruptionPhaseState: Equatable {
    let phase: InterruptionPhase
    let isPlaying: Bool
}

private struct InterruptionObservation {
    let trace: [InterruptionPhaseState]
    let resumeDisplacement: TimeInterval
}

private struct InterruptionControls {
    let play: () -> Void
    let pause: () -> Void
    let isPlaying: () -> Bool
    let position: () -> TimeInterval
}

private final class AVQueuePlayerInterruptionAdapter: @unchecked Sendable {
    private let player: AVQueuePlayer
    private var pausedByInterruption = false
    private var cancellable: AnyCancellable?

    init(player: AVQueuePlayer) {
        self.player = player
        cancellable = NotificationCenter.default
            .publisher(for: AVAudioSession.interruptionNotification)
            .sink { [weak self] notification in
                self?.handle(notification)
            }
    }

    private func handle(_ notification: Notification) {
        guard
            let rawType = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
            let type = AVAudioSession.InterruptionType(rawValue: rawType)
        else {
            return
        }

        switch type {
        case .began:
            pausedByInterruption = player.rate > 0
            if pausedByInterruption {
                player.pause()
            }
        case .ended:
            let rawOptions = notification.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt
            let shouldResume = rawOptions.map {
                AVAudioSession.InterruptionOptions(rawValue: $0).contains(.shouldResume)
            } ?? false
            let resume = pausedByInterruption && shouldResume
            pausedByInterruption = false
            if resume {
                player.play()
            }
        @unknown default:
            return
        }
    }
}

private final class InterruptionRates: @unchecked Sendable {
    private let lock = NSLock()
    private var rates: [Float] = []

    func record(_ rate: Float) {
        lock.lock()
        defer { lock.unlock() }
        rates.append(rate)
    }

    func reset() {
        lock.lock()
        defer { lock.unlock() }
        rates.removeAll()
    }

    func snapshot() -> [Float] {
        lock.lock()
        defer { lock.unlock() }
        return rates
    }
}

private struct InterruptionFactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
