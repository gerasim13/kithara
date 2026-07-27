import AVFAudio
import Combine
import Foundation
import Kithara
import Testing

@Suite("LABA iOS traps", .serialized)
struct LabaIOSTraps {}

extension LabaIOSTraps {
    @Test("LABA-415 pauses and resumes for an audio-session interruption")
    func laba415InterruptionResume() async throws {
        let audioSession = AVAudioSession.sharedInstance()
        try audioSession.setCategory(.playback)
        try audioSession.setActive(true)

        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("laba-415-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )
        let player = KitharaPlayer(config: .init(cacheDir: cacheURL.path))
        let item = KitharaPlayerItem(
            url: try TestServerFixture.asset("test.mp3").absoluteString
        )
        let rates = Laba415Rates()
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
        try await waitFor415Fact("fixture playback to advance") {
            player.currentTime > 0.1
        }
        try #require(
            player.currentRate > 0,
            "LABA-415 precondition: fixture playback did not start"
        )

        // The player reacts across the FFI boundary, so reading the rate in the
        // same breath as posting the notification would race a correct
        // implementation and keep this trap red even once it is fixed.
        rates.reset()
        postInterruption(.began)
        let paused = await reached415Fact { player.currentRate == 0 }
        #expect(
            paused,
            """
            LABA-415: playback did not pause after AVAudioSession posted \
            interruption-began; rate=\(player.currentRate), \
            published=\(rates.snapshot())
            """
        )

        rates.reset()
        postInterruption(.ended, options: .shouldResume)
        let resumed = await reached415Fact { player.currentRate > 0 }
        #expect(
            resumed,
            """
            LABA-415: playback did not resume after AVAudioSession ended the \
            interruption with .shouldResume; rate=\(player.currentRate), \
            published=\(rates.snapshot())
            """
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
    private func reached415Fact(_ condition: () -> Bool) async -> Bool {
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

    private func waitFor415Fact(
        _ description: String,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(30))
        while !condition() {
            guard clock.now < deadline else {
                throw Laba415FactTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

private final class Laba415Rates: @unchecked Sendable {
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

private struct Laba415FactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
