package com.kithara.example.ui.player

import com.kithara.PlayerStatus
import com.kithara.TrackStatus

internal data class PlaylistEntry(
    val id: String,
    val name: String,
    val url: String,
    val trackStatus: TrackStatus? = null,
    val duration: Double? = null,
)

internal data class PlayerUiState(
    val currentTimeSeconds: Float = 0f,
    val currentTrackId: String? = null,
    val durationSeconds: Float? = null,
    val errorMessage: String? = null,
    val isPlaying: Boolean = false,
    val isSeeking: Boolean = false,
    val playlist: List<PlaylistEntry> = emptyList(),
    val selectedRate: Float = DefaultRate,
    val availableRates: List<Float> = AvailableRates,
    val status: PlayerStatus = PlayerStatus.Unknown,
    val url: String = "",
    val volume: Float = 1.0f,
    val isMuted: Boolean = false,
    val eqGains: List<Float> = emptyList(),
    val crossfadeDuration: Float = 5.0f,
    val abrIsAuto: Boolean = true,
    val selectedVariantIndex: UInt? = null,
    val discoveredVariants: List<Pair<UInt, String>> = emptyList(),
    val currentVariantLabel: String? = null,
) {

    val currentTrackIndex: Int = playlist.indexOfFirst { it.id == currentTrackId }

    val trackTitle: String
        get() = playlist.getOrNull(currentTrackIndex)?.name ?: "No Track"

    companion object {
        private const val DefaultRate = 1.0f
        private val AvailableRates: List<Float> = listOf(0.5f, 0.75f, 1.0f, 1.25f, 1.5f, 2.0f)
    }
}
