package com.kithara.ffi

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class FfiHlsSourceSettingsTest {
    @Test
    fun batchSizeConstructorRemainsAvailable() {
        val legacy = FfiHlsSourceSettings(downloadBatchSize = 6u)
        val configured = FfiHlsSourceSettings(
            sizeProbeMethod = FfiSizeProbeMethod.RANGE_GET,
            downloadBatchSize = 6u,
        )

        assertEquals(6u, legacy.downloadBatchSize)
        assertNull(legacy.sizeProbeMethod)
        assertEquals(FfiSizeProbeMethod.RANGE_GET, configured.sizeProbeMethod)
    }
}
