package com.kithara.ffi

/** Creates HLS source settings with the previously available batch-size option. */
fun FfiHlsSourceSettings(downloadBatchSize: UInt?): FfiHlsSourceSettings =
    FfiHlsSourceSettings(sizeProbeMethod = null, downloadBatchSize = downloadBatchSize)
