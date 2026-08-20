import AVFAudio
import Foundation
import Kithara
import Testing

@Suite("Integration regressions (iOS)", .serialized)
struct IntegrationRegressionsIOS {}

extension IntegrationRegressionsIOS {
    @Test("An interruption pauses playback")
    func interruptionPausesPlayback() async throws {
        try await withPlayingFixture { player in
            postInterruption(.began)
            try await waitForInterruptionFact("playback to pause after interruption began") {
                player.currentRate == 0
            }
        }
    }

    @Test("An interruption that ends with permission resumes playback")
    func interruptionEndedWithPermissionResumesPlayback() async throws {
        try await withPlayingFixture { player in
            postInterruption(.began)
            try await waitForInterruptionFact("playback to pause after interruption began") {
                player.currentRate == 0
            }

            let origin = player.currentTime
            postInterruption(.ended, options: .shouldResume)
            try await waitForInterruptionFact("playback to resume and advance after shouldResume") {
                player.currentRate > 0 && player.currentTime >= origin + 0.15
            }
        }
    }

    @Test("An interruption that ends without permission keeps playback paused")
    func interruptionEndedWithoutPermissionKeepsPlaybackPaused() async throws {
        try await withPlayingFixture { player in
            postInterruption(.began)
            try await waitForInterruptionFact("playback to pause after interruption began") {
                player.currentRate == 0
            }

            postInterruption(.ended)
            let resumed = await reachedInterruptionFact(for: .seconds(1)) {
                player.currentRate > 0
            }
            #expect(!resumed, "Playback resumed after an interruption ended without shouldResume")
        }
    }

    /// iOS delivers consecutive `began` notifications without an `ended`
    /// between them — an incoming call that is never answered raises the
    /// interruption more than once. The permission to resume belongs to the
    /// interruption as a whole, so a repeated `began` must not withdraw it.
    @Test("A repeated interruption-began keeps the permission to resume")
    func repeatedInterruptionBeganKeepsResumePermission() async throws {
        try await withPlayingFixture { player in
            postInterruption(.began)
            try await waitForInterruptionFact("playback to pause after the first began") {
                player.currentRate == 0
            }

            postInterruption(.began)

            let origin = player.currentTime
            postInterruption(.ended, options: .shouldResume)
            try await waitForInterruptionFact("playback to resume and advance after shouldResume") {
                player.currentRate > 0 && player.currentTime >= origin + 0.15
            }
        }
    }

    /// The framework must never hold a `play()` back on its own account: the
    /// system decides whether playback resumes by itself, the user decides
    /// whether it resumes at all.
    ///
    /// A simulator cannot take the audio output away the way a phone call
    /// does — measured on Xcode 26.6 / iOS 26.3.1, `setActive(false)` under a
    /// running stream leaves the clock advancing — so this pins the framework's
    /// own transport logic, not the recovery of a torn-down output. Recovery is
    /// only observable on a device.
    @Test("A public play resumes playback after an interruption")
    func publicPlayResumesPlaybackAfterAnInterruption() async throws {
        try await withPlayingFixture { player in
            postInterruption(.began)
            try await waitForInterruptionFact("playback to pause after interruption began") {
                player.currentRate == 0
            }
            postInterruption(.ended)

            let origin = player.currentTime
            player.play()
            try await waitForInterruptionFact("public play to resume and advance playback") {
                player.currentRate > 0 && player.currentTime >= origin + 0.15
            }

            player.pause()
            try await waitForInterruptionFact("public pause to pause playback") {
                player.currentRate == 0
            }
        }
    }

    private func withPlayingFixture(
        _ body: (KitharaPlayer) async throws -> Void
    ) async throws {
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
        defer {
            player.stop()
            try? audioSession.setActive(false, options: .notifyOthersOnDeactivation)
            try? FileManager.default.removeItem(at: cacheURL)
        }

        try player.insert(item)
        player.play()
        try await waitForInterruptionFact("fixture playback to advance") {
            player.currentRate > 0 && player.currentTime > 0.1
        }

        try await body(player)
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

    private func reachedInterruptionFact(
        for timeout: Duration = .seconds(5),
        _ condition: () -> Bool
    ) async -> Bool {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
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

private struct InterruptionFactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
