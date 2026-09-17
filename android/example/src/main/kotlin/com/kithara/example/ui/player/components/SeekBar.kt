package com.kithara.example.ui.player.components

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import com.kithara.example.ui.player.PlayerScreenEvent
import com.kithara.example.ui.theme.AccentGold
import com.kithara.example.ui.theme.KitharaMuted
import com.kithara.example.ui.theme.PanelBorder
import com.kithara.example.ui.theme.PrimaryText
import com.kithara.example.ui.theme.SecondaryText

@Composable
internal fun SeekBar(
    currentTimeSeconds: Float,
    durationSeconds: Float?,
    enabled: Boolean,
    isSeeking: Boolean,
    onEvent: (PlayerScreenEvent) -> Unit,
) {
    Column(modifier = Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Slider(
            value = currentTimeSeconds,
            onValueChange = { value ->
                if (!isSeeking) onEvent(PlayerScreenEvent.SeekStarted)
                onEvent(PlayerScreenEvent.SeekChanged(value))
            },
            modifier = Modifier.fillMaxWidth(),
            valueRange = 0f..(durationSeconds ?: 1f),
            enabled = enabled,
            onValueChangeFinished = { onEvent(PlayerScreenEvent.SeekFinished) },
            colors = SliderDefaults.colors(
                thumbColor = PrimaryText,
                activeTrackColor = AccentGold,
                inactiveTrackColor = PanelBorder,
                disabledActiveTrackColor = PanelBorder,
                disabledInactiveTrackColor = PanelBorder,
                disabledThumbColor = KitharaMuted,
            ),
        )
        Row(modifier = Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(
                text = formatTime(currentTimeSeconds),
                color = SecondaryText,
                style = MaterialTheme.typography.labelLarge,
                fontFamily = FontFamily.Monospace,
            )
            Box(modifier = Modifier.weight(1f))
            Text(
                text = formatTime(durationSeconds),
                color = SecondaryText,
                style = MaterialTheme.typography.labelLarge,
                fontFamily = FontFamily.Monospace,
            )
        }
    }
}
