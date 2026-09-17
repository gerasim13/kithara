package com.kithara.example.ui.player.components

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.kithara.example.ui.player.PlayerScreenEvent
import com.kithara.example.ui.theme.AccentGold
import com.kithara.example.ui.theme.CardBackground
import com.kithara.example.ui.theme.KitharaMuted
import com.kithara.example.ui.theme.PanelBorder
import com.kithara.example.ui.theme.PrimaryText
import com.kithara.example.ui.theme.SecondaryText
import kotlin.math.exp
import kotlin.math.ln
import kotlin.math.roundToInt

@Composable
internal fun EqualizerTab(
    eqGains: List<Float>,
    onEvent: (PlayerScreenEvent) -> Unit,
    modifier: Modifier = Modifier,
) {
    if (eqGains.isEmpty()) {
        Box(modifier = modifier, contentAlignment = Alignment.Center) {
            Text(text = "EQ is not available", color = KitharaMuted)
        }
        return
    }

    Card(
        modifier = modifier.fillMaxWidth(),
        shape = RoundedCornerShape(10.dp),
        colors = CardDefaults.cardColors(containerColor = CardBackground),
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    text = "EQ",
                    style = MaterialTheme.typography.labelLarge,
                    color = PrimaryText,
                    fontWeight = FontWeight.SemiBold,
                )
                Box(modifier = Modifier.weight(1f))
                TextButton(
                    onClick = { onEvent(PlayerScreenEvent.EqResetClick) },
                    contentPadding = PaddingValues(horizontal = 8.dp, vertical = 0.dp),
                ) {
                    Text(
                        text = "Reset",
                        color = SecondaryText,
                        style = MaterialTheme.typography.labelSmall,
                    )
                }
            }
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                eqGains.forEachIndexed { index, gain ->
                    EqBand(
                        gain = gain,
                        label = eqBandLabel(index, eqGains.size),
                        onChange = { value ->
                            onEvent(PlayerScreenEvent.EqBandChanged(index, value))
                        },
                        modifier = Modifier.weight(1f),
                    )
                }
            }
        }
    }
}

@Composable
private fun EqBand(
    gain: Float,
    label: String,
    onChange: (Float) -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier = modifier,
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Box(
            modifier = Modifier.size(width = 28.dp, height = 140.dp),
            contentAlignment = Alignment.Center,
        ) {
            Slider(
                value = gain.coerceIn(EQ_GAIN_MIN, EQ_GAIN_MAX),
                onValueChange = onChange,
                valueRange = EQ_GAIN_MIN..EQ_GAIN_MAX,
                modifier = Modifier
                    .requiredSize(width = 140.dp, height = 28.dp)
                    .rotate(-90f),
                colors = SliderDefaults.colors(
                    thumbColor = PrimaryText,
                    activeTrackColor = AccentGold,
                    inactiveTrackColor = PanelBorder,
                ),
            )
        }
        Text(
            text = label,
            color = KitharaMuted,
            fontSize = 9.sp,
            maxLines = 1,
        )
    }
}

private fun eqBandLabel(band: Int, total: Int): String {
    val logMin = ln(30.0)
    val logMax = ln(18000.0)
    val frac = if (total > 1) band.toDouble() / (total - 1).toDouble() else 0.0
    val freq = exp(logMin + frac * (logMax - logMin))
    return if (freq >= 1000.0) "${(freq / 1000.0).roundToInt()}k" else "${freq.roundToInt()}"
}

private const val EQ_GAIN_MIN: Float = -24f
private const val EQ_GAIN_MAX: Float = 6f
