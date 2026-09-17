package com.kithara.example.ui.player

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBars
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import com.kithara.PlayerStatus
import com.kithara.example.ui.player.components.EqualizerTab
import com.kithara.example.ui.player.components.ErrorBanner
import com.kithara.example.ui.player.components.NowPlayingCard
import com.kithara.example.ui.player.components.PlayerHeader
import com.kithara.example.ui.player.components.PlaylistTab
import com.kithara.example.ui.player.components.RateSelector
import com.kithara.example.ui.player.components.SeekBar
import com.kithara.example.ui.player.components.SettingsTab
import com.kithara.example.ui.player.components.TabPills
import com.kithara.example.ui.player.components.TransportControls
import com.kithara.example.ui.player.components.UrlInputRow
import com.kithara.example.ui.player.components.VolumeControl
import com.kithara.example.ui.theme.KitharaBackground
import com.kithara.example.ui.theme.KitharaTheme

@Composable
internal fun PlayerScreen(
    uiState: PlayerUiState,
    onEvent: (PlayerScreenEvent) -> Unit,
    modifier: Modifier = Modifier,
) {
    val duration = uiState.durationSeconds ?: 0f
    val sliderEnabled = duration > 0f
    val sliderValue = if (sliderEnabled) {
        uiState.currentTimeSeconds.coerceIn(0f, duration)
    } else {
        0f
    }

    var selectedTabIndex by remember { mutableIntStateOf(0) }
    val tabs = listOf("Playlist", "EQ", "Settings")

    val systemBarsPadding = WindowInsets.systemBars.asPaddingValues()
    Column(
        modifier = modifier
            .fillMaxSize()
            .background(KitharaBackground)
            .padding(
                top = systemBarsPadding.calculateTopPadding() + 20.dp,
                bottom = systemBarsPadding.calculateBottomPadding() + 20.dp,
                start = 20.dp,
                end = 20.dp,
            ),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        PlayerHeader(
            isPlaying = uiState.isPlaying,
            status = uiState.status,
            hasTracks = uiState.playlist.isNotEmpty(),
        )
        UrlInputRow(
            url = uiState.url,
            onEvent = onEvent,
        )
        NowPlayingCard(
            trackTitle = uiState.trackTitle,
            variantLabel = uiState.currentVariantLabel,
        )
        SeekBar(
            currentTimeSeconds = sliderValue,
            durationSeconds = uiState.durationSeconds,
            enabled = sliderEnabled,
            isSeeking = uiState.isSeeking,
            onEvent = onEvent,
        )
        TransportControls(
            isPlaying = uiState.isPlaying,
            onEvent = onEvent,
        )
        RateSelector(
            selectedRate = uiState.selectedRate,
            availableRates = uiState.availableRates,
            onEvent = onEvent,
        )
        VolumeControl(
            volume = uiState.volume,
            isMuted = uiState.isMuted,
            onEvent = onEvent,
        )

        TabPills(
            tabs = tabs,
            selectedIndex = selectedTabIndex,
            onSelect = { selectedTabIndex = it },
        )

        Box(modifier = Modifier.weight(1f)) {
            when (selectedTabIndex) {
                0 -> PlaylistTab(
                    playlist = uiState.playlist,
                    currentTrackId = uiState.currentTrackId,
                    onEvent = onEvent,
                    modifier = Modifier.fillMaxSize(),
                )
                1 -> EqualizerTab(
                    eqGains = uiState.eqGains,
                    onEvent = onEvent,
                    modifier = Modifier.fillMaxSize(),
                )
                2 -> SettingsTab(
                    crossfadeDuration = uiState.crossfadeDuration,
                    abrIsAuto = uiState.abrIsAuto,
                    selectedVariantIndex = uiState.selectedVariantIndex,
                    discoveredVariants = uiState.discoveredVariants,
                    onEvent = onEvent,
                    modifier = Modifier.fillMaxSize(),
                )
            }
        }

        uiState.errorMessage?.let { ErrorBanner(message = it) }
    }
}

@Preview(
    showBackground = true,
    backgroundColor = 0xFF1A1A2E,
    heightDp = 900,
)
@Composable
private fun PlayerScreenPreview() {
    val playlist = listOf(
        PlaylistEntry(id = "preview-1", url = "", name = "song.mp3", duration = 215.0),
        PlaylistEntry(id = "preview-2", url = "", name = "another-long-track-name-that-gets-truncated.mp3", duration = 184.0),
        PlaylistEntry(id = "preview-3", url = "", name = "third-track.mp3", duration = null),
    )

    KitharaTheme {
        PlayerScreen(
            uiState = PlayerUiState(
                currentTimeSeconds = 42f,
                durationSeconds = 180f,
                selectedRate = 1f,
                status = PlayerStatus.ReadyToPlay,
                url = "",
                playlist = playlist,
                currentTrackId = playlist.first().id,
                eqGains = List(10) { 0f },
                currentVariantLabel = "256 kbps",
            ),
            onEvent = {},
        )
    }
}
