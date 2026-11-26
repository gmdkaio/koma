package dev.koma.app.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.koma.app.ui.theme.LocalKomaPalette

@Composable
fun KomaApp(
    state: KomaUiState,
    viewModel: KomaViewModel,
) {
    val palette = LocalKomaPalette.current
    val snackbarHostState = remember { SnackbarHostState() }

    val authLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.StartIntentSenderForResult(),
    ) { result ->
        viewModel.handleAuthActivityResult(result.resultCode, result.data)
    }

    LaunchedEffect(state.pendingAuthResolution) {
        val sender = state.pendingAuthResolution ?: return@LaunchedEffect
        viewModel.clearPendingAuthResolution()
        authLauncher.launch(IntentSenderRequest.Builder(sender).build())
    }

    LaunchedEffect(state.toastMessage) {
        val msg = state.toastMessage ?: return@LaunchedEffect
        snackbarHostState.showSnackbar(msg)
        viewModel.clearToast()
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(palette.bg),
    ) {
        if (!state.isSignedIn) {
            SignInScreen(
                theme = state.theme,
                onSelectTheme = viewModel::setTheme,
                onSignIn = viewModel::startSignIn,
            )
        } else {
            GalleryScreen(
                state = state,
                viewModel = viewModel,
            )

            val viewerIdx = state.viewerIndex
            val visible = state.visibleImages
            AnimatedVisibility(
                visible = viewerIdx != null && visible.isNotEmpty(),
                enter = fadeIn(tween(140)) + scaleIn(initialScale = 0.96f, animationSpec = tween(160)),
                exit = fadeOut(tween(120)) + scaleOut(targetScale = 0.97f, animationSpec = tween(120)),
            ) {
                val safeIdx = (viewerIdx ?: 0).coerceIn(0, (visible.lastIndex).coerceAtLeast(0))
                if (visible.isNotEmpty()) {
                    ViewerScreen(
                        images = visible,
                        initialIndex = safeIdx,
                        imageLoader = viewModel.imageLoader,
                        onClose = viewModel::closeViewer,
                        onShare = { viewModel.shareImages(listOf(it)) },
                        onSave = { viewModel.saveImagesToGallery(listOf(it)) },
                        onMove = { viewModel.requestMove(listOf(it.id)) },
                        onDelete = { viewModel.requestDelete(listOf(it.id)) },
                    )
                }
            }
        }

        if (state.showCreateChannel) {
            CreateChannelDialog(
                onDismiss = { viewModel.setShowCreateChannel(false) },
                onCreate = { name -> viewModel.createChannel(name) },
            )
        }

        if (state.pendingMoveIds.isNotEmpty()) {
            MoveChannelSheet(
                count = state.pendingMoveIds.size,
                channels = state.channels,
                onDismiss = viewModel::dismissMove,
                onSelectChannel = viewModel::confirmMoveTo,
                onCreateAndMove = { rawName ->
                    viewModel.createChannel(rawName) { ch ->
                        viewModel.confirmMoveTo(ch)
                    }
                },
            )
        }

        if (state.pendingDeleteIds.isNotEmpty()) {
            DeleteConfirmDialog(
                count = state.pendingDeleteIds.size,
                onDismiss = viewModel::dismissDelete,
                onConfirm = viewModel::confirmDelete,
            )
        }

        if (state.pendingShareUris.isNotEmpty() && state.isSignedIn) {
            IncomingShareSheet(
                uris = state.pendingShareUris,
                channels = state.channels,
                imageLoader = viewModel.imageLoader,
                onDismiss = viewModel::dismissIncomingShare,
                onSelectBucket = { bucket ->
                    viewModel.uploadToBucket(bucket, state.pendingShareUris)
                },
                onSelectChannel = { ch ->
                    viewModel.uploadUrisToChannel(ch, state.pendingShareUris)
                },
                onCreateAndUpload = { rawName ->
                    val uris = state.pendingShareUris
                    viewModel.createChannel(rawName) { ch ->
                        viewModel.uploadUrisToChannel(ch, uris)
                    }
                },
            )
        }

        AnimatedVisibility(
            visible = state.busyMessage != null,
            enter = fadeIn(),
            exit = fadeOut(),
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .navigationBarsPadding()
                .padding(bottom = 84.dp),
        ) {
            Surface(
                color = palette.surfaceElevated,
                contentColor = palette.fg,
                shape = RoundedCornerShape(10.dp),
                shadowElevation = 8.dp,
            ) {
                Row(
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    CircularProgressIndicator(
                        modifier = Modifier.size(16.dp),
                        strokeWidth = 2.dp,
                        color = palette.accent,
                    )
                    Spacer(Modifier.width(10.dp))
                    Text(
                        text = state.busyMessage.orEmpty(),
                        fontSize = 13.sp,
                        color = palette.fg,
                    )
                }
            }
        }

        SnackbarHost(
            hostState = snackbarHostState,
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .navigationBarsPadding()
                .padding(bottom = 12.dp),
        )
    }
}
