import Combine
import Foundation
import Kithara
import Testing

extension IntegrationRegressionsIOS {
    @Test("ABR storm applies the final manual variant and playback continues")
    func abrStormAppliesFinalManualVariantAndPlaybackContinues() async throws {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("abr-storm-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: cacheURL,
            withIntermediateDirectories: true
        )

        let player = KitharaPlayer(config: .init(store: AssetStore(root: cacheURL.path)))
        let item = KitharaPlayerItem(
            url: try await throttledAbrMasterURL().absoluteString,
            abrMode: .manual(variantIndex: 0)
        )
        let facts = AbrFacts()
        let variantsCancellable = item.variantsDiscovered.sink { variants in
            facts.recordDiscovered(variants)
        }
        let selectedCancellable = item.variantSelected.sink { variant in
            facts.recordSelected(variant)
        }
        let appliedCancellable = item.variantApplied.sink { variant in
            facts.recordApplied(variant)
        }
        let errorCancellable = item.error.sink { error in
            facts.recordFailure(error)
        }
        defer {
            player.stop()
            try? FileManager.default.removeItem(at: cacheURL)
            _ = variantsCancellable
            _ = selectedCancellable
            _ = appliedCancellable
            _ = errorCancellable
        }

        try player.insert(item)
        player.play()
        try await waitForAbrFact("four HLS variants to be discovered", facts: facts) {
            facts.discovered.count == 4
        }
        try #require(
            facts.discovered.map(\.index) == [0, 1, 2, 3],
            "precondition: expected variant indexes 0...3, got \(facts.discovered.map(\.index))"
        )
        try await waitForAbrFact("initial Manual(0) to reach the decoder", facts: facts) {
            facts.applied.contains(0)
        }
        try #require(
            facts.firstApplied == 0,
            "precondition: initial applied variant must be 0, got \(facts.applied)"
        )

        let caps: [Double] = [96_000, 256_000, 512_000, 512_000]
        for round in 0..<10 {
            let variantIndex = round % facts.discovered.count
            let mode: AbrMode = round.isMultiple(of: 2)
                ? .manual(variantIndex: variantIndex)
                : .auto
            player.setAbrMode(mode)
            player.updatePeakBitrate(
                wifi: caps[variantIndex],
                cellular: caps[variantIndex]
            )
            try await Task.sleep(nanoseconds: 100_000_000)
        }

        let finalEventOffset = facts.events.count
        player.setAbrMode(.manual(variantIndex: 3))
        player.updatePeakBitrate(wifi: 0, cellular: 0)

        try await waitForAbrFact("final Manual(3) to be selected", facts: facts) {
            facts.events.dropFirst(finalEventOffset).contains(.selected(3))
        }
        try await waitForAbrFact("final Manual(3) to reach the decoder", facts: facts) {
            let events = facts.events.dropFirst(finalEventOffset)
            guard let selected = events.firstIndex(of: .selected(3)) else {
                return false
            }
            return events[events.index(after: selected)...].contains(.applied(3))
        }
        let appliedTime = player.currentTime
        try #require(
            appliedTime.isFinite,
            "currentTime was not finite when Manual(3) reached the decoder"
        )
        try await waitForAbrFact(
            "playback to advance five seconds after Manual(3) was applied",
            facts: facts,
            deadline: .seconds(60)
        ) {
            player.currentTime >= appliedTime + 5
        }
        try #require(
            facts.failure == nil,
            "ABR storm ended with a terminal item error: \(facts.failure ?? "none")"
        )
    }

    private func throttledAbrMasterURL() async throws -> URL {
        let specs = [
            AbrVariantSpec(
                bandwidth: 66_005,
                codecs: "mp4a.40.2",
                playlist: "index-slq-a1.m3u8",
                initialization: "init-slq-a1.mp4"
            ),
            AbrVariantSpec(
                bandwidth: 134_107,
                codecs: "mp4a.40.2",
                playlist: "index-smq-a1.m3u8",
                initialization: "init-smq-a1.mp4"
            ),
            AbrVariantSpec(
                bandwidth: 269_930,
                codecs: "mp4a.40.2",
                playlist: "index-shq-a1.m3u8",
                initialization: "init-shq-a1.mp4"
            ),
            AbrVariantSpec(
                bandwidth: 988_758,
                codecs: "fLaC",
                playlist: "index-slossless-a1.m3u8",
                initialization: "init-slossless-a1.mp4"
            ),
        ]

        var playlistURLs: [URL] = []
        for spec in specs {
            let sourceURL = try TestServerFixture.asset("hls/\(spec.playlist)")
            let playlist = try await abrPlaylistText(at: sourceURL)
            let rewritten = try rewriteAbrPlaylist(playlist, spec: spec)
            let handle = try await TestServerFixture.registerBehavior(
                .init(
                    content: .bytes(
                        Data(rewritten.utf8),
                        contentType: "application/vnd.apple.mpegurl"
                    ),
                    delivery: .throttle(chunk: 16, delayMilliseconds: 50)
                )
            )
            playlistURLs.append(handle.childURL(spec.playlist))
        }

        var master = "#EXTM3U\n"
        for (spec, playlistURL) in zip(specs, playlistURLs) {
            master += """
            #EXT-X-STREAM-INF:PROGRAM-ID=1,BANDWIDTH=\(spec.bandwidth),\
            CODECS="\(spec.codecs)",AVERAGE-BANDWIDTH=\(spec.bandwidth)
            \(playlistURL.absoluteString)

            """
        }
        let masterHandle = try await TestServerFixture.registerBehavior(
            .init(
                content: .bytes(
                    Data(master.utf8),
                    contentType: "application/vnd.apple.mpegurl"
                ),
                delivery: .normal
            )
        )
        return masterHandle.childURL("master.m3u8")
    }

    private func abrPlaylistText(at url: URL) async throws -> String {
        let (data, response) = try await URLSession.shared.data(from: url)
        let http = try #require(
            response as? HTTPURLResponse,
            "precondition: \(url.lastPathComponent) returned a non-HTTP response"
        )
        try #require(
            http.statusCode == 200,
            """
            precondition: \(url.lastPathComponent) returned HTTP \
            \(http.statusCode)
            """
        )
        return try #require(
            String(data: data, encoding: .utf8),
            "precondition: \(url.lastPathComponent) was not UTF-8"
        )
    }

    private func rewriteAbrPlaylist(
        _ playlist: String,
        spec: AbrVariantSpec
    ) throws -> String {
        let initializationURL = try TestServerFixture.asset(
            "hls/\(spec.initialization)"
        )
        var rewritten: [String] = []

        for substring in playlist.split(
            separator: "\n",
            omittingEmptySubsequences: false
        ) {
            let line = String(substring)
            if line.hasPrefix("#EXT-X-MAP:") {
                rewritten.append(
                    line.replacingOccurrences(
                        of: spec.initialization,
                        with: initializationURL.absoluteString
                    )
                )
            } else if !line.isEmpty && !line.hasPrefix("#") {
                rewritten.append(
                    try TestServerFixture.asset("hls/\(line)").absoluteString
                )
            } else {
                rewritten.append(line)
            }
        }
        return rewritten.joined(separator: "\n")
    }

    private func waitForAbrFact(
        _ description: String,
        facts: AbrFacts,
        deadline duration: Duration = .seconds(30),
        condition: () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: duration)
        while true {
            if let failure = facts.failure {
                throw AbrTerminalFailure(description: description, failure: failure)
            }
            if condition() {
                return
            }
            guard clock.now < deadline else {
                throw AbrFactTimeout(description)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }
}

private struct AbrVariantSpec {
    let bandwidth: Int
    let codecs: String
    let playlist: String
    let initialization: String
}

private enum AbrEvent: Equatable {
    case selected(Int)
    case applied(Int)
}

private final class AbrFacts: @unchecked Sendable {
    private let lock = NSLock()
    private var discoveredVariants: [Variant] = []
    private var variantEvents: [AbrEvent] = []
    private var failureDescription: String?

    var discovered: [Variant] {
        lock.lock()
        defer { lock.unlock() }
        return discoveredVariants
    }

    var events: [AbrEvent] {
        lock.lock()
        defer { lock.unlock() }
        return variantEvents
    }

    var applied: [Int] {
        lock.lock()
        defer { lock.unlock() }
        return variantEvents.compactMap { event in
            if case let .applied(index) = event {
                return index
            }
            return nil
        }
    }

    var firstApplied: Int? {
        lock.lock()
        defer { lock.unlock() }
        return variantEvents.lazy.compactMap { event in
            if case let .applied(index) = event {
                return index
            }
            return nil
        }.first
    }

    var failure: String? {
        lock.lock()
        defer { lock.unlock() }
        return failureDescription
    }

    func recordDiscovered(_ variants: [Variant]) {
        lock.lock()
        defer { lock.unlock() }
        discoveredVariants = variants
    }

    func recordSelected(_ variant: Variant) {
        lock.lock()
        defer { lock.unlock() }
        variantEvents.append(.selected(variant.index))
    }

    func recordApplied(_ variant: Variant) {
        lock.lock()
        defer { lock.unlock() }
        variantEvents.append(.applied(variant.index))
    }

    func recordFailure(_ error: Error) {
        lock.lock()
        defer { lock.unlock() }
        failureDescription = String(describing: error)
    }
}

private struct AbrTerminalFailure: Error, CustomStringConvertible {
    let description: String

    init(description: String, failure: String) {
        self.description = "Item failed while waiting for \(description): \(failure)"
    }
}

private struct AbrFactTimeout: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = "Timed out waiting for \(description)"
    }
}
