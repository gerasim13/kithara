import KitharaFFI

/// Process-wide Kithara audio host lifecycle.
public enum KitharaHost {
    /// Settings fixed for the lifetime of the process audio host.
    public struct Configuration: Sendable {
        public var sampleRateHint: UInt32
        public var outputBlockFrames: UInt32?

        public init(sampleRateHint: UInt32 = 44_100, outputBlockFrames: UInt32? = nil) {
            self.sampleRateHint = sampleRateHint
            self.outputBlockFrames = outputBlockFrames
        }
    }

    /// Initialize the process audio host exactly once, before creating players.
    public static func initialize(configuration: Configuration = .init()) throws {
        do {
            try initializeHost(
                config: FfiHostConfig(
                    sampleRateHint: configuration.sampleRateHint,
                    outputBlockFrames: configuration.outputBlockFrames
                )
            )
        } catch let error as FfiError {
            throw KitharaError(ffi: error)
        }
    }
}
