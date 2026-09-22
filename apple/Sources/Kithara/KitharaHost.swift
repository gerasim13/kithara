import KitharaFFI

/// Process-wide Kithara audio host lifecycle.
public enum KitharaHost {
    /// Settings fixed for the lifetime of the process audio host.
    public struct Configuration: Sendable {
        public var sampleRateHint: UInt32
        public var outputBlockFrames: UInt32?
        public var limiter: FfiLimiterConfig

        public init(
            sampleRateHint: UInt32 = defaultHostConfig().sampleRateHint,
            outputBlockFrames: UInt32? = defaultHostConfig().outputBlockFrames,
            limiter: FfiLimiterConfig = defaultHostConfig().limiter
        ) {
            self.sampleRateHint = sampleRateHint
            self.outputBlockFrames = outputBlockFrames
            self.limiter = limiter
        }
    }

    /// Initialize the process audio host exactly once, before creating players.
    public static func initialize(configuration: Configuration = .init()) throws {
        do {
            try initializeHost(
                config: FfiHostConfig(
                    sampleRateHint: configuration.sampleRateHint,
                    outputBlockFrames: configuration.outputBlockFrames,
                    limiter: configuration.limiter
                )
            )
        } catch let error as FfiError {
            throw KitharaError(ffi: error)
        }
    }
}
