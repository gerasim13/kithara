package com.kithara.example.ui.player

internal sealed interface PlayerScreenEvent {
    data class UrlChanged(val url: String) : PlayerScreenEvent
    data object AddClick : PlayerScreenEvent
    data object PlayPauseClick : PlayerScreenEvent
    data object PrevClick : PlayerScreenEvent
    data object NextClick : PlayerScreenEvent
    data class TrackClick(val trackId: String) : PlayerScreenEvent
    data class RateClick(val rate: Float) : PlayerScreenEvent
    data object SeekStarted : PlayerScreenEvent
    data class SeekChanged(val value: Float) : PlayerScreenEvent
    data object SeekFinished : PlayerScreenEvent
    data class RemoveTrackClick(val trackId: String) : PlayerScreenEvent
    data class VolumeChanged(val volume: Float) : PlayerScreenEvent
    data object MuteClick : PlayerScreenEvent
    data class EqBandChanged(val bandIndex: Int, val gain: Float) : PlayerScreenEvent
    data object EqResetClick : PlayerScreenEvent
    data class CrossfadeChanged(val durationSeconds: Float) : PlayerScreenEvent
    data class AbrChanged(val variantIndex: UInt?) : PlayerScreenEvent
}
