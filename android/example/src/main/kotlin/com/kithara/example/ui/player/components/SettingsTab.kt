package com.kithara.example.ui.player.components

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.kithara.example.ui.player.PlayerScreenEvent
import com.kithara.example.ui.theme.AccentGold
import com.kithara.example.ui.theme.PanelBorder
import com.kithara.example.ui.theme.PrimaryText
import com.kithara.example.ui.theme.SecondaryText

@Composable
internal fun SettingsTab(
    crossfadeDuration: Float,
    abrIsAuto: Boolean,
    selectedVariantIndex: UInt?,
    discoveredVariants: List<Pair<UInt, String>>,
    onEvent: (PlayerScreenEvent) -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier = modifier
            .padding(vertical = 16.dp)
            .fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(
                text = "Quality",
                style = MaterialTheme.typography.labelLarge,
                color = PrimaryText,
            )
            QualityChips(
                abrIsAuto = abrIsAuto,
                selectedVariantIndex = selectedVariantIndex,
                discoveredVariants = discoveredVariants,
                onEvent = onEvent,
            )
        }
        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(
                text = "Crossfade",
                style = MaterialTheme.typography.labelLarge,
                color = PrimaryText,
            )
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Slider(
                    value = crossfadeDuration.coerceIn(0f, CROSSFADE_MAX_SECONDS),
                    onValueChange = { value ->
                        onEvent(PlayerScreenEvent.CrossfadeChanged(value))
                    },
                    valueRange = 0f..CROSSFADE_MAX_SECONDS,
                    modifier = Modifier.weight(1f),
                    colors = SliderDefaults.colors(
                        thumbColor = PrimaryText,
                        activeTrackColor = AccentGold,
                        inactiveTrackColor = PanelBorder,
                    ),
                )
                Text(
                    text = "%.1fs".format(crossfadeDuration),
                    color = SecondaryText,
                    style = MaterialTheme.typography.labelLarge,
                    modifier = Modifier.width(48.dp),
                    textAlign = TextAlign.End,
                    fontFamily = FontFamily.Monospace,
                )
            }
        }
    }
}

@Composable
private fun QualityChips(
    abrIsAuto: Boolean,
    selectedVariantIndex: UInt?,
    discoveredVariants: List<Pair<UInt, String>>,
    onEvent: (PlayerScreenEvent) -> Unit,
) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        PillButton(
            text = "Auto",
            selected = abrIsAuto,
            onClick = { onEvent(PlayerScreenEvent.AbrChanged(null)) },
        )
        discoveredVariants.forEach { (index, label) ->
            val selected = !abrIsAuto && selectedVariantIndex == index
            PillButton(
                text = label,
                selected = selected,
                onClick = { onEvent(PlayerScreenEvent.AbrChanged(index)) },
            )
        }
    }
}

private const val CROSSFADE_MAX_SECONDS: Float = 8f
