import Combine
import Foundation
import Kithara
import Testing

extension LabaIOSTraps {
    @Test("LABA-420 repeat mode is writable, readable, and observable")
    func laba420RepeatMode() async throws {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("laba-420-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )

        let player = KitharaPlayer(config: .init(cacheDir: cacheURL.path))
        let events = Laba420RepeatEvents()
        let cancellable = player.eventPublisher.sink { event in
            if case let .repeatModeChanged(mode) = event {
                events.record(mode)
            }
        }
        defer {
            player.stop()
            try? FileManager.default.removeItem(at: cacheURL)
            _ = cancellable
        }

        player.repeatMode = .one
        try await waitFor420Fact("repeatModeChanged(.one)") {
            events.contains(.one)
        }
        try await waitFor420Fact("repeatMode getter to report .one") {
            player.repeatMode == .one
        }

        #expect(events.contains(.one))
        #expect(player.repeatMode == .one)
    }

    private func waitFor420Fact(
        _ description: String,
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(5))
        while !condition() {
            guard clock.now < deadline else {
                throw Laba420FactTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

private final class Laba420RepeatEvents: @unchecked Sendable {
    private let lock = NSLock()
    private var modes: [RepeatMode] = []

    func record(_ mode: RepeatMode) {
        lock.lock()
        defer { lock.unlock() }
        modes.append(mode)
    }

    func contains(_ mode: RepeatMode) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return modes.contains(mode)
    }
}

private struct Laba420FactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
