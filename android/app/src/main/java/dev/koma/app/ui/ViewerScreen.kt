package dev.koma.app.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.DriveFileMove
import androidx.compose.material.icons.rounded.DeleteOutline
import androidx.compose.material.icons.rounded.Download
import androidx.compose.material.icons.rounded.Share
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil3.ImageLoader
import coil3.compose.AsyncImage
import coil3.request.ImageRequest
import dev.koma.app.data.DriveImage
import dev.koma.app.data.formatBytes

@Composable
fun ViewerScreen(
    images: List<DriveImage>,
    initialIndex: Int,
    imageLoader: ImageLoader,
    onClose: () -> Unit,
    onShare: (DriveImage) -> Unit,
    onSave: (DriveImage) -> Unit,
    onMove: (DriveImage) -> Unit,
    onDelete: (DriveImage) -> Unit,
) {
    BackHandler(onBack = onClose)

    val pagerState = rememberPagerState(initialPage = initialIndex) { images.size }
    var showChrome by remember { mutableStateOf(true) }
    val current = images.getOrNull(pagerState.currentPage) ?: return

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(Color(0xFF06070A)),
    ) {
        HorizontalPager(
            state = pagerState,
            modifier = Modifier.fillMaxSize(),
        ) { page ->
            val img = images[page]
            ZoomableFullImage(
                image = img,
                imageLoader = imageLoader,
                onTap = { showChrome = !showChrome },
            )
        }

        AnimatedVisibility(
            visible = showChrome,
            enter = fadeIn(),
            exit = fadeOut(),
            modifier = Modifier.align(Alignment.TopCenter),
        ) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .background(Color(0xCC0A0B0F))
                    .statusBarsPadding()
                    .padding(horizontal = 6.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(onClick = onClose) {
                    Icon(
                        Icons.AutoMirrored.Rounded.ArrowBack,
                        contentDescription = "Back",
                        tint = Color.White,
                    )
                }
                Column(modifier = Modifier.weight(1f)) {
                    Text(
                        text = current.filename,
                        color = Color.White,
                        fontSize = 14.sp,
                        fontWeight = FontWeight.SemiBold,
                        fontFamily = FontFamily.Monospace,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    val meta = buildString {
                        append("#${current.channelSlug}")
                        if (current.width > 0 && current.height > 0) {
                            append(" · ${current.width}×${current.height}")
                        }
                        if (current.sizeBytes > 0) {
                            append(" · ${formatBytes(current.sizeBytes)}")
                        }
                    }
                    Text(
                        text = meta,
                        color = Color(0xFFA0A8B8),
                        fontSize = 11.5.sp,
                        fontFamily = FontFamily.Monospace,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                Spacer(Modifier.width(8.dp))
                Text(
                    text = "${pagerState.currentPage + 1}/${images.size}",
                    color = Color(0xFFA0A8B8),
                    fontSize = 12.sp,
                    fontFamily = FontFamily.Monospace,
                    modifier = Modifier.padding(end = 10.dp),
                )
            }
        }

        AnimatedVisibility(
            visible = showChrome,
            enter = fadeIn(),
            exit = fadeOut(),
            modifier = Modifier.align(Alignment.BottomCenter),
        ) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .background(Color(0xCC0A0B0F))
                    .navigationBarsPadding()
                    .padding(horizontal = 16.dp, vertical = 10.dp),
                horizontalArrangement = Arrangement.SpaceEvenly,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                ViewerActionButton(
                    icon = Icons.Rounded.Share,
                    label = "Share",
                    tint = Color.White,
                    onClick = { onShare(current) },
                )
                ViewerActionButton(
                    icon = Icons.Rounded.Download,
                    label = "Save",
                    tint = Color.White,
                    onClick = { onSave(current) },
                )
                ViewerActionButton(
                    icon = Icons.AutoMirrored.Rounded.DriveFileMove,
                    label = "Move",
                    tint = Color.White,
                    onClick = { onMove(current) },
                )
                ViewerActionButton(
                    icon = Icons.Rounded.DeleteOutline,
                    label = "Delete",
                    tint = Color(0xFFF87171),
                    onClick = { onDelete(current) },
                )
            }
        }
    }
}

@Composable
private fun ZoomableFullImage(
    image: DriveImage,
    imageLoader: ImageLoader,
    onTap: () -> Unit,
) {
    val context = LocalContext.current
    var scale by remember(image.id) { mutableFloatStateOf(1f) }
    var offset by remember(image.id) { mutableStateOf(Offset.Zero) }

    val fullUrl = "https://www.googleapis.com/drive/v3/files/${image.id}?alt=media"
    val request = remember(image.id) {
        ImageRequest.Builder(context)
            .data(fullUrl)
            .memoryCacheKey("full_${image.id}")
            .diskCacheKey("full_${image.id}")
            .placeholderMemoryCacheKey("thumb_${image.id}")
            .build()
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .pointerInput(image.id) {
                detectTapGestures(
                    onTap = { onTap() },
                    onDoubleTap = {
                        if (scale > 1.05f) {
                            scale = 1f
                            offset = Offset.Zero
                        } else {
                            scale = 2.5f
                        }
                    },
                )
            }
            .pointerInput(image.id) {
                awaitEachGesture {
                    awaitFirstDown(requireUnconsumed = false)
                    do {
                        val event = awaitPointerEvent()
                        val pressedCount = event.changes.count { it.pressed }
                        if (pressedCount >= 2 || scale > 1.02f) {
                            val zoomChange = event.calculateZoom()
                            val panChange = event.calculatePan()
                            if (zoomChange != 1f || (scale > 1.02f && panChange != Offset.Zero)) {
                                val nextScale = (scale * zoomChange).coerceIn(1f, 4.5f)
                                scale = nextScale
                                offset = if (nextScale <= 1.02f) Offset.Zero else offset + panChange
                                event.changes.forEach { if (it.positionChanged()) it.consume() }
                            }
                        }
                    } while (event.changes.any { it.pressed })
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        AsyncImage(
            model = request,
            imageLoader = imageLoader,
            contentDescription = image.filename,
            contentScale = ContentScale.Fit,
            modifier = Modifier
                .fillMaxSize()
                .graphicsLayer {
                    scaleX = scale
                    scaleY = scale
                    translationX = offset.x
                    translationY = offset.y
                },
        )
    }
}

@Composable
private fun ViewerActionButton(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    label: String,
    tint: Color,
    onClick: () -> Unit,
) {
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        IconButton(onClick = onClick) {
            Icon(icon, contentDescription = label, tint = tint)
        }
        Text(text = label, color = tint, fontSize = 11.sp)
    }
}
