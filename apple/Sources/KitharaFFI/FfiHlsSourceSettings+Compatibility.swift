extension FfiHlsSourceSettings {
    /// Creates HLS source settings with the previously available batch-size option.
    public init(downloadBatchSize: UInt32?) {
        self.init(lookAheadBytes: nil, sizeProbeMethod: nil, downloadBatchSize: downloadBatchSize)
    }

    /// Creates HLS source settings with the previously available probe and batch options.
    public init(sizeProbeMethod: FfiSizeProbeMethod?, downloadBatchSize: UInt32?) {
        self.init(lookAheadBytes: nil, sizeProbeMethod: sizeProbeMethod, downloadBatchSize: downloadBatchSize)
    }
}
