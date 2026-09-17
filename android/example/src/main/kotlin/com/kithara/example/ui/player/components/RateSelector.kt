package com.kithara.example.ui.player.components

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.kithara.example.ui.player.PlayerScreenEvent

@Composable
internal fun RateSelector(
    selectedRate: Float,
    availableRates: List<Float>,
    onEvent: (PlayerScreenEvent) -> Unit,
) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        availableRates.forEach { rate ->
            PillButton(
                text = rateLabel(rate),
                selected = selectedRate == rate,
                onClick = { onEvent(PlayerScreenEvent.RateClick(rate)) },
                modifier = Modifier.weight(1f),
            )
        }
    }
}

private fun rateLabel(rate: Float): String =
    if (rate == rate.toInt().toFloat()) "${rate.toInt()}x" else "${rate}x"
