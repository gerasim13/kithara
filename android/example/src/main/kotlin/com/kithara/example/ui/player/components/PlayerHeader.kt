package com.kithara.example.ui.player.components

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.kithara.PlayerStatus
import com.kithara.example.R
import com.kithara.example.ui.theme.AccentGold
import com.kithara.example.ui.theme.KitharaDanger
import com.kithara.example.ui.theme.KitharaMuted
import com.kithara.example.ui.theme.KitharaSuccess
import com.kithara.example.ui.theme.PanelBackground
import com.kithara.example.ui.theme.PrimaryText
import com.kithara.example.ui.theme.SecondaryText

@Composable
internal fun PlayerHeader(isPlaying: Boolean, status: PlayerStatus, hasTracks: Boolean) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = stringResource(R.string.app_title),
            style = MaterialTheme.typography.headlineMedium,
            color = if (isPlaying) AccentGold else PrimaryText,
            fontWeight = FontWeight.Bold,
        )
        Text(
            text = stringResource(R.string.demo_label),
            style = MaterialTheme.typography.bodyMedium,
            color = SecondaryText,
            modifier = Modifier.padding(start = 8.dp, top = 10.dp),
        )
        Box(modifier = Modifier.weight(1f))
        StatusBadge(status = status, isPlaying = isPlaying, hasTracks = hasTracks)
    }
}

@Composable
private fun StatusBadge(status: PlayerStatus, isPlaying: Boolean, hasTracks: Boolean) {
    val statusColor = when {
        status == PlayerStatus.Failed -> KitharaDanger
        !hasTracks -> KitharaMuted
        isPlaying -> KitharaSuccess
        else -> AccentGold
    }
    val statusText = when {
        status == PlayerStatus.Failed -> stringResource(R.string.status_failed)
        !hasTracks -> stringResource(R.string.status_not_ready)
        isPlaying -> stringResource(R.string.status_playing)
        else -> stringResource(R.string.status_idle)
    }

    Row(
        modifier = Modifier
            .clip(CircleShape)
            .background(PanelBackground)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            modifier = Modifier
                .size(8.dp)
                .clip(CircleShape)
                .background(statusColor),
        )
        Text(
            text = statusText,
            color = SecondaryText,
            style = MaterialTheme.typography.labelLarge,
        )
    }
}
