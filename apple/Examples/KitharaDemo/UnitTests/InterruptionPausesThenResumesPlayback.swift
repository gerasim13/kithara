import AVFAudio
import Foundation
import Kithara
import Testing

@Suite("Integration regressions (iOS)", .serialized)
struct IntegrationRegressionsIOS {}

extension IntegrationRegressionsIOS {
    @Test("An interruption resumes only when allowed and keeps controls responsive")
    func interruptionResumesOnlyWhenAllowed() async throws {
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

        postInterruption(.began)
        try await waitForInterruptionFact("playback to pause after interruption began") {
            player.currentRate == 0
        }

        let automaticResumeOrigin = player.currentTime
        postInterruption(.ended, options: .shouldResume)
        try await waitForInterruptionFact("playback to resume and advance after shouldResume") {
            player.currentRate > 0 && player.currentTime >= automaticResumeOrigin + 0.15
        }

        postInterruption(.began)
        try await waitForInterruptionFact("playback to pause before no-resume ended") {
            player.currentRate == 0
        }

        postInterruption(.ended)
        let resumedWithoutPermission = await reachedInterruptionFact(for: .seconds(1)) {
            player.currentRate > 0
        }
        #expect(
            !resumedWithoutPermission,
            "Playback resumed after interruption ended without shouldResume"
        )

        let manualResumeOrigin = player.currentTime
        player.play()
        try await waitForInterruptionFact("public play to resume and advance playback") {
            player.currentRate > 0 && player.currentTime >= manualResumeOrigin + 0.15
        }

        player.pause()
        try await waitForInterruptionFact("public pause to pause playback") {
            player.currentRate == 0
        }
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
