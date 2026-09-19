package com.kithara.example.ui.player.components

internal fun formatTime(seconds: Float?): String {
    val value = seconds ?: return "--:--"
    val totalSeconds = value.toInt()
    return "%d:%02d".format(totalSeconds / 60, totalSeconds % 60)
}

internal fun formatDuration(seconds: Double?): String =
    formatTime(seconds?.toFloat())
