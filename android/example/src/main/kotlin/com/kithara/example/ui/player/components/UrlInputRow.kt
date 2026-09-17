package com.kithara.example.ui.player.components

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.kithara.example.R
import com.kithara.example.ui.player.PlayerScreenEvent
import com.kithara.example.ui.theme.AccentGold
import com.kithara.example.ui.theme.KitharaBackground
import com.kithara.example.ui.theme.KitharaMuted
import com.kithara.example.ui.theme.PanelBackground
import com.kithara.example.ui.theme.PanelBorder
import com.kithara.example.ui.theme.PrimaryText

@Composable
internal fun UrlInputRow(url: String, onEvent: (PlayerScreenEvent) -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        OutlinedTextField(
            value = url,
            onValueChange = { onEvent(PlayerScreenEvent.UrlChanged(it)) },
            modifier = Modifier.weight(1f),
            placeholder = {
                Text(text = stringResource(R.string.audio_url_hint), color = KitharaMuted)
            },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
            colors = TextFieldDefaults.colors(
                focusedContainerColor = PanelBackground,
                unfocusedContainerColor = PanelBackground,
                disabledContainerColor = PanelBackground,
                focusedIndicatorColor = PanelBorder,
                unfocusedIndicatorColor = PanelBorder,
                cursorColor = AccentGold,
                focusedTextColor = PrimaryText,
                unfocusedTextColor = PrimaryText,
            ),
            shape = RoundedCornerShape(16.dp),
        )
        Button(
            onClick = { onEvent(PlayerScreenEvent.AddClick) },
            enabled = url.isNotBlank(),
            shape = RoundedCornerShape(16.dp),
            colors = ButtonDefaults.buttonColors(
                containerColor = AccentGold,
                contentColor = KitharaBackground,
                disabledContainerColor = AccentGold.copy(alpha = 0.35f),
                disabledContentColor = KitharaBackground.copy(alpha = 0.7f),
            ),
        ) {
            Text(text = stringResource(R.string.add_action))
        }
    }
}
