import Foundation

enum TestServerFixture {
    static let environmentVariable = "KITHARA_TEST_SERVER_URL"

    static var baseURL: URL? {
        guard
            let value = ProcessInfo.processInfo.environment[environmentVariable],
            !value.isEmpty,
            let url = URL(string: value),
            let scheme = url.scheme,
            ["http", "https"].contains(scheme),
            url.host != nil
        else {
            return nil
        }
        return url
    }

    enum FixtureError: Error, LocalizedError {
        case missingBaseURL
        case invalidFixturePath(String)
        case invalidResponse(URL)
        case unexpectedStatus(URL, Int, String)

        var errorDescription: String? {
            switch self {
            case .missingBaseURL:
                "\(TestServerFixture.environmentVariable) is unset or is not a valid HTTP URL"
            case let .invalidFixturePath(path):
                "Could not build a test-server URL for fixture path \(path)"
            case let .invalidResponse(url):
                "Test server returned a non-HTTP response for \(url.absoluteString)"
            case let .unexpectedStatus(url, status, body):
                "Test server returned HTTP \(status) for \(url.absoluteString): \(body)"
            }
        }
    }

    enum Content: Encodable, Sendable {
        case htmlError
        case status(code: UInt16)
        case bytes(Data, contentType: String?)
        /// Serve a fixture the server already has on disk. Preferred over
        /// ``bytes(_:contentType:)`` for anything sizeable — uploading a
        /// multi-megabyte body is rejected by the request-body limit.
        case asset(name: String)
        /// Serve one body the fixture generator produces, named the way
        /// ``signal(_:)`` names it. Generated bodies have no path under
        /// `assets/`, so they need their own case.
        case signal(name: String)

        private enum CodingKeys: String, CodingKey {
            case kind
            case code
            case base64
            case contentType = "content_type"
            case name
        }

        func encode(to encoder: Encoder) throws {
            var container = encoder.container(keyedBy: CodingKeys.self)
            switch self {
            case .htmlError:
                try container.encode("html_error", forKey: .kind)
            case let .status(code):
                try container.encode("status", forKey: .kind)
                try container.encode(code, forKey: .code)
            case let .bytes(data, contentType):
                try container.encode("bytes", forKey: .kind)
                try container.encode(data.base64EncodedString(), forKey: .base64)
                try container.encodeIfPresent(contentType, forKey: .contentType)
            case let .asset(name):
                try container.encode("asset", forKey: .kind)
                try container.encode(name, forKey: .name)
            case let .signal(name):
                try container.encode("signal", forKey: .kind)
                try container.encode(name, forKey: .name)
            }
        }
    }

    enum Delivery: Encodable, Sendable {
        case normal
        case range
        case earlyClose(afterBytes: Int)
        case stallAfter(afterBytes: Int)
        case throttle(chunk: Int, delayMilliseconds: UInt64)

        private enum CodingKeys: String, CodingKey {
            case kind
            case afterBytes = "after_bytes"
            case chunk
            case delayMilliseconds = "delay_ms"
        }

        func encode(to encoder: Encoder) throws {
            var container = encoder.container(keyedBy: CodingKeys.self)
            switch self {
            case .normal:
                try container.encode("normal", forKey: .kind)
            case .range:
                try container.encode("range", forKey: .kind)
            case let .earlyClose(afterBytes):
                try container.encode("early_close", forKey: .kind)
                try container.encode(afterBytes, forKey: .afterBytes)
            case let .stallAfter(afterBytes):
                try container.encode("stall_after", forKey: .kind)
                try container.encode(afterBytes, forKey: .afterBytes)
            case let .throttle(chunk, delayMilliseconds):
                try container.encode("throttle", forKey: .kind)
                try container.encode(chunk, forKey: .chunk)
                try container.encode(delayMilliseconds, forKey: .delayMilliseconds)
            }
        }
    }

    struct Behavior: Encodable, Sendable {
        let content: Content
        let delivery: Delivery
    }

    struct BehaviorHandle: Sendable {
        let token: String
        let url: URL

        func childURL(_ path: String) -> URL {
            path.split(separator: "/", omittingEmptySubsequences: true)
                .reduce(url) { result, component in
                    result.appendingPathComponent(String(component))
                }
        }
    }

    private struct NetworkRequest: Encodable {
        let online: Bool
    }

    private struct TokenResponse: Decodable {
        let token: String
    }

    static func requireBaseURL() throws -> URL {
        guard let baseURL else {
            throw FixtureError.missingBaseURL
        }
        return baseURL
    }

    static func url(_ path: String) throws -> URL {
        var baseURL = try requireBaseURL()
        baseURL.appendPathComponent("")
        let relativePath = path.drop(while: { $0 == "/" })
        guard let result = URL(string: String(relativePath), relativeTo: baseURL)?.absoluteURL else {
            throw FixtureError.invalidFixturePath(path)
        }
        return result
    }

    static func asset(_ name: String) throws -> URL {
        try url("assets/\(name.trimmingCharacters(in: CharacterSet(charactersIn: "/")))")
    }

    /// URL of one generated body, named `{accessor}.{ext}`.
    static func signal(_ name: String) throws -> URL {
        try url("signal/\(name.trimmingCharacters(in: CharacterSet(charactersIn: "/")))")
    }

    static func streamHQ(_ name: String) throws -> URL {
        var components = URLComponents(url: try url("streamhq"), resolvingAgainstBaseURL: false)
        let trimmedName = name.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        components?.queryItems = [URLQueryItem(name: "name", value: trimmedName)]
        guard let result = components?.url else {
            throw FixtureError.invalidFixturePath(name)
        }
        return result
    }

    /// A single-variant HLS master whose media segments arrive at roughly the
    /// rate they are consumed.
    ///
    /// The fixture server answers over loopback, so a player drains the whole
    /// track into its store within a second of `play()`. A test that then
    /// takes the network away observes nothing — playback runs on from what is
    /// already stored. Pacing the segments, while the playlists stay
    /// immediate, bounds how far ahead the player can get, which is what makes
    /// an outage observable at all.
    static func pacedHlsMasterURL(
        chunk: Int = 3072,
        delayMilliseconds: UInt64 = 250
    ) async throws -> URL {
        let variant = "index-slq-a1.m3u8"
        let initialization = "init-slq-a1.mp4"
        let playlist = try await fixtureText(at: asset("hls/\(variant)"))

        var rewritten: [String] = []
        for substring in playlist.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(substring)
            if line.hasPrefix("#EXT-X-MAP:") {
                rewritten.append(
                    line.replacingOccurrences(
                        of: initialization,
                        with: try asset("hls/\(initialization)").absoluteString
                    )
                )
            } else if !line.isEmpty, !line.hasPrefix("#") {
                let segment = try await registerBehavior(
                    .init(
                        content: .asset(name: "hls/\(line)"),
                        delivery: .throttle(
                            chunk: chunk,
                            delayMilliseconds: delayMilliseconds
                        )
                    )
                )
                rewritten.append(segment.childURL(line).absoluteString)
            } else {
                rewritten.append(line)
            }
        }

        let media = try await registerBehavior(
            .init(
                content: .bytes(
                    Data(rewritten.joined(separator: "\n").utf8),
                    contentType: "application/vnd.apple.mpegurl"
                ),
                delivery: .normal
            )
        )
        let master = """
        #EXTM3U
        #EXT-X-STREAM-INF:PROGRAM-ID=1,BANDWIDTH=66005,CODECS="mp4a.40.2"
        \(media.childURL(variant).absoluteString)

        """
        let masterHandle = try await registerBehavior(
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

    private static func fixtureText(at url: URL) async throws -> String {
        let (data, response) = try await URLSession.shared.data(from: url)
        guard let http = response as? HTTPURLResponse else {
            throw FixtureError.invalidResponse(url)
        }
        guard http.statusCode == 200 else {
            let body = String(data: data, encoding: .utf8) ?? "<non-UTF-8 body>"
            throw FixtureError.unexpectedStatus(url, http.statusCode, body)
        }
        guard let text = String(data: data, encoding: .utf8) else {
            throw FixtureError.invalidResponse(url)
        }
        return text
    }

    static func setNetwork(online: Bool) async throws {
        _ = try await post(
            NetworkRequest(online: online),
            to: "control/network",
            expectedStatus: 204
        )
    }

    static func registerBehavior(_ behavior: Behavior) async throws -> BehaviorHandle {
        let data = try await post(
            behavior,
            to: "control/behavior",
            expectedStatus: 200
        )
        let token = try JSONDecoder().decode(TokenResponse.self, from: data).token
        return BehaviorHandle(token: token, url: try url("behavior/\(token)"))
    }

    private static func post<Body: Encodable>(
        _ body: Body,
        to path: String,
        expectedStatus: Int
    ) async throws -> Data {
        let endpoint = try url(path)
        var request = URLRequest(url: endpoint)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(body)

        let (data, response) = try await URLSession.shared.data(for: request)
        guard let response = response as? HTTPURLResponse else {
            throw FixtureError.invalidResponse(endpoint)
        }
        guard response.statusCode == expectedStatus else {
            let responseBody = String(data: data, encoding: .utf8) ?? "<non-UTF-8 body>"
            throw FixtureError.unexpectedStatus(endpoint, response.statusCode, responseBody)
        }
        return data
    }
}
