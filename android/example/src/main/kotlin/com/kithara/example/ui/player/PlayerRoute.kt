package com.kithara.example.ui.player

import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle

@Composable
internal fun PlayerRoute(viewModel: PlayerViewModel) {
    val uiState by viewModel.uiState.collectAsStateWithLifecycle()

    PlayerScreen(uiState = uiState, onEvent = viewModel::onEvent)
}
