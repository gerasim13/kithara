package com.kithara.example.ui.player.components

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.VolumeOff
import androidx.compose.material.icons.automirrored.rounded.VolumeUp
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.kithara.example.ui.player.PlayerScreenEvent
import com.kithara.example.ui.theme.AccentGold
import com.kithara.example.ui.theme.KitharaMuted
import com.kithara.example.ui.theme.PanelBorder
import com.kithara.example.ui.theme.PrimaryText
import com.kithara.example.ui.theme.SecondaryText

@Composable
internal fun VolumeControl(volume: Float, isMuted: Boolean, onEvent: (PlayerScreenEvent) -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        IconButton(onClick = { onEvent(PlayerScreenEvent.MuteClick) }) {
            Icon(
                imageVector = if (isMuted) Icons.AutoMirrored.Rounded.VolumeOff else Icons.AutoMirrored.Rounded.VolumeUp,
                contentDescription = null,
                tint = if (isMuted) KitharaMuted else AccentGold,
            )
        }
        Slider(
            value = if (isMuted) 0f else volume,
            onValueChange = { onEvent(PlayerScreenEvent.VolumeChanged(it)) },
            modifier = Modifier.weight(1f),
            valueRange = 0f..1f,
            colors = SliderDefaults.colors(
                thumbColor = PrimaryText,
                activeTrackColor = AccentGold,
                inactiveTrackColor = PanelBorder,
            ),
        )
        Text(
            text = "%d%%".format(((if (isMuted) 0f else volume) * 100f).toInt()),
            color = SecondaryText,
            style = MaterialTheme.typography.labelLarge,
            modifier = Modifier.width(40.dp),
            textAlign = TextAlign.End,
        )
    }
}
