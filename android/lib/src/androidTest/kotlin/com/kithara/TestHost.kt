package com.kithara

import android.content.Context

object TestHost {
    private var initialized = false

    @Synchronized
    fun initialize(context: Context, logLevel: LogLevel = LogLevel.Warn) {
        if (initialized) return
        try {
            Kithara.initialize(context, logLevel)
        } catch (_: KitharaError.AlreadyInitialized) {
            // Another instrumentation suite initialized the one process host.
        }
        initialized = true
    }
}
