package com.kithara.example.ui.player.components

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Delete
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.kithara.TrackStatus
import com.kithara.example.ui.player.PlaylistEntry
import com.kithara.example.R
import com.kithara.example.ui.player.PlayerScreenEvent
import com.kithara.example.ui.theme.AccentGold
import com.kithara.example.ui.theme.KitharaDanger
import com.kithara.example.ui.theme.KitharaMuted
import com.kithara.example.ui.theme.KitharaWarning
import com.kithara.example.ui.theme.PanelBackground
import com.kithara.example.ui.theme.PrimaryText
import com.kithara.example.ui.theme.SecondaryText

@Composable
internal fun PlaylistTab(
    playlist: List<PlaylistEntry>,
    currentTrackId: String?,
    onEvent: (PlayerScreenEvent) -> Unit,
    modifier: Modifier = Modifier,
) {
    if (playlist.isEmpty()) {
        Box(modifier = modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
            Text(
                text = stringResource(R.string.playlist_empty),
                color = KitharaMuted,
                style = MaterialTheme.typography.bodyMedium,
            )
        }
        return
    }

    LazyColumn(
        modifier = modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        itemsIndexed(playlist, key = { _, entry -> entry.id }) { index, entry ->
            PlaylistItem(
                index = index,
                entry = entry,
                isCurrent = entry.id == currentTrackId,
                onClick = { onEvent(PlayerScreenEvent.TrackClick(entry.id)) },
                onRemove = { onEvent(PlayerScreenEvent.RemoveTrackClick(entry.id)) },
            )
        }
    }
}

@Composable
private fun PlaylistItem(
    index: Int,
    entry: PlaylistEntry,
    isCurrent: Boolean,
    onClick: () -> Unit,
    onRemove: () -> Unit,
) {
    val dismissState = rememberSwipeToDismissBoxState()
    val currentOnRemove by rememberUpdatedState(onRemove)
    LaunchedEffect(dismissState.currentValue) {
        if (dismissState.currentValue == SwipeToDismissBoxValue.EndToStart) {
            currentOnRemove()
        }
    }

    SwipeToDismissBox(
        state = dismissState,
        enableDismissFromStartToEnd = false,
        enableDismissFromEndToStart = true,
        backgroundContent = {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .clip(RoundedCornerShape(8.dp))
                    .background(KitharaDanger)
                    .padding(horizontal = 16.dp),
                contentAlignment = Alignment.CenterEnd,
            ) {
                Icon(
                    imageVector = Icons.Rounded.Delete,
                    contentDescription = null,
                    tint = PrimaryText,
                    modifier = Modifier.size(20.dp),
                )
            }
        },
    ) {
        PlaylistRow(
            index = index,
            entry = entry,
            isCurrent = isCurrent,
            onClick = onClick,
        )
    }
}

@Composable
private fun PlaylistRow(
    index: Int,
    entry: PlaylistEntry,
    isCurrent: Boolean,
    onClick: () -> Unit,
) {
    val statusColor = trackStatusColor(entry.trackStatus)
    val background = if (isCurrent) AccentGold.copy(alpha = 0.18f).compositeOver(PanelBackground) else PanelBackground
    val indexColor = statusColor ?: if (isCurrent) AccentGold else KitharaMuted
    val nameColor = statusColor ?: if (isCurrent) PrimaryText else SecondaryText

    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(background)
            .clickable(onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = "%02d".format(index + 1),
            color = indexColor,
            style = MaterialTheme.typography.labelMedium,
            fontFamily = FontFamily.Monospace,
        )
        Text(
            text = entry.name,
            color = nameColor,
            style = MaterialTheme.typography.bodyMedium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        Text(
            text = formatDuration(entry.duration),
            color = KitharaMuted,
            style = MaterialTheme.typography.labelSmall,
            fontFamily = FontFamily.Monospace,
        )
    }
}

private fun trackStatusColor(status: TrackStatus?): Color? = when (status) {
    is TrackStatus.Slow -> KitharaWarning
    is TrackStatus.Failed -> KitharaDanger
    else -> null
}
