package com.kithara.example.ui.player.components

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.FastForward
import androidx.compose.material.icons.rounded.FastRewind
import androidx.compose.material.icons.rounded.Pause
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.kithara.example.ui.player.PlayerScreenEvent
import com.kithara.example.ui.theme.AccentGold
import com.kithara.example.ui.theme.KitharaBackground
import com.kithara.example.ui.theme.PrimaryText

@Composable
internal fun TransportControls(isPlaying: Boolean, onEvent: (PlayerScreenEvent) -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(onClick = { onEvent(PlayerScreenEvent.PrevClick) }) {
            Icon(
                imageVector = Icons.Rounded.FastRewind,
                contentDescription = null,
                tint = PrimaryText,
                modifier = Modifier.size(32.dp),
            )
        }
        FilledTonalButton(
            onClick = { onEvent(PlayerScreenEvent.PlayPauseClick) },
            modifier = Modifier
                .padding(horizontal = 24.dp)
                .size(64.dp),
            shape = CircleShape,
            colors = ButtonDefaults.filledTonalButtonColors(
                containerColor = AccentGold,
                contentColor = KitharaBackground,
            ),
        ) {
            Icon(
                imageVector = if (isPlaying) Icons.Rounded.Pause else Icons.Rounded.PlayArrow,
                contentDescription = null,
                modifier = Modifier.size(28.dp),
            )
        }
        IconButton(onClick = { onEvent(PlayerScreenEvent.NextClick) }) {
            Icon(
                imageVector = Icons.Rounded.FastForward,
                contentDescription = null,
                tint = PrimaryText,
                modifier = Modifier.size(32.dp),
            )
        }
    }
}
