extension FfiHlsSourceSettings {
    /// Creates HLS source settings with the previously available batch-size option.
    public init(downloadBatchSize: UInt32?) {
        self.init(sizeProbeMethod: nil, downloadBatchSize: downloadBatchSize)
    }
}
