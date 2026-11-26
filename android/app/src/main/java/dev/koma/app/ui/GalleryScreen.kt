package dev.koma.app.ui

import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGesturesAfterLongPress
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.staggeredgrid.LazyStaggeredGridState
import androidx.compose.foundation.lazy.staggeredgrid.LazyVerticalStaggeredGrid
import androidx.compose.foundation.lazy.staggeredgrid.StaggeredGridCells
import androidx.compose.foundation.lazy.staggeredgrid.itemsIndexed
import androidx.compose.foundation.lazy.staggeredgrid.rememberLazyStaggeredGridState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.DriveFileMove
import androidx.compose.material.icons.automirrored.rounded.Logout
import androidx.compose.material.icons.automirrored.rounded.Send
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.ChevronRight
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.DeleteOutline
import androidx.compose.material.icons.rounded.Download
import androidx.compose.material.icons.rounded.ExpandMore
import androidx.compose.material.icons.rounded.FolderOpen
import androidx.compose.material.icons.rounded.Home
import androidx.compose.material.icons.rounded.Menu
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.SelectAll
import androidx.compose.material.icons.rounded.Share
import androidx.compose.material.icons.rounded.SwapVert
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil3.ImageLoader
import coil3.compose.AsyncImage
import coil3.request.ImageRequest
import coil3.request.allowRgb565
import coil3.size.Precision
import coil3.size.Size
import dev.koma.app.R
import dev.koma.app.data.ActiveTab
import dev.koma.app.data.DriveImage
import dev.koma.app.data.DriveTransferFile
import dev.koma.app.data.FileBucket
import dev.koma.app.data.KomaTheme
import dev.koma.app.data.formatBytes
import dev.koma.app.ui.theme.LocalKomaPalette
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

@Composable
fun GalleryScreen(
    state: KomaUiState,
    viewModel: KomaViewModel,
) {
    val palette = LocalKomaPalette.current
    val drawerState = rememberDrawerState(DrawerValue.Closed)
    val gridState = rememberLazyStaggeredGridState()
    val scope = rememberCoroutineScope()
    val haptic = LocalHapticFeedback.current
    val density = LocalDensity.current

    val visibleImages = remember(state.allImages, state.activeTab, state.sortOrder) {
        state.visibleImages
    }
    val latestImages by rememberUpdatedState(visibleImages)
    val selecting = state.selectedIds.isNotEmpty()
    val allSelected = visibleImages.isNotEmpty() && state.selectedIds.size == visibleImages.size

    var autoScrollSpeed by remember { mutableFloatStateOf(0f) }
    LaunchedEffect(autoScrollSpeed) {
        if (autoScrollSpeed == 0f) return@LaunchedEffect
        while (isActive && autoScrollSpeed != 0f) {
            gridState.scrollBy(autoScrollSpeed)
            delay(16L)
        }
    }

    val docPicker = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.OpenMultipleDocuments(),
    ) { uris ->
        if (uris.isNotEmpty()) {
            val bucket = when (state.activeTab) {
                ActiveTab.Throwaway -> FileBucket.THROWAWAY
                ActiveTab.Files -> FileBucket.FILES
                else -> FileBucket.FILES
            }
            viewModel.uploadToBucket(bucket, uris)
        }
    }

    val photoPicker = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.PickMultipleVisualMedia(maxItems = 30),
    ) { uris ->
        if (uris.isNotEmpty()) {
            val active = state.activeChannel
            if (active != null) {
                viewModel.uploadUrisToChannel(active, uris)
            } else {
                viewModel.handleIncomingShare(uris)
            }
        }
    }

    BackHandler(enabled = selecting || drawerState.isOpen) {
        when {
            drawerState.isOpen -> scope.launch { drawerState.close() }
            selecting -> viewModel.clearSelection()
        }
    }

    ModalNavigationDrawer(
        drawerState = drawerState,
        drawerContent = {
            ChannelDrawer(
                state = state,
                onSelectTab = { tab ->
                    viewModel.selectTab(tab)
                    scope.launch { drawerState.close() }
                },
                onNewChannel = {
                    scope.launch { drawerState.close() }
                    viewModel.setShowCreateChannel(true)
                },
                onToggleChannels = viewModel::toggleChannelsCollapsed,
                onSelectTheme = viewModel::setTheme,
                onSignOut = {
                    scope.launch { drawerState.close() }
                    viewModel.signOut()
                },
            )
        },
    ) {
        Box(
            modifier = Modifier
                .fillMaxSize()
                .background(palette.bg),
        ) {
            val activeBucket = when (state.activeTab) {
                ActiveTab.Throwaway -> FileBucket.THROWAWAY
                ActiveTab.Files -> FileBucket.FILES
                else -> null
            }
            if (activeBucket != null) {
                LaunchedEffect(activeBucket) {
                    viewModel.refreshTransfersOnly()
                    if (activeBucket == FileBucket.THROWAWAY) {
                        while (isActive) {
                            delay(5_000L)
                            viewModel.refreshTransfersOnly()
                        }
                    }
                }
                val bucketFiles = if (activeBucket == FileBucket.THROWAWAY) {
                    state.throwawayFiles
                } else {
                    state.savedFiles
                }
                FileBucketContent(
                    bucket = activeBucket,
                    files = bucketFiles,
                    isSyncing = state.isSyncing,
                    onMenu = { scope.launch { drawerState.open() } },
                    onUpload = { docPicker.launch(arrayOf("*/*")) },
                    onSync = viewModel::syncNow,
                    onSave = { viewModel.receiveTransferFile(it, shareInsteadOfSave = false) },
                    onShare = { viewModel.receiveTransferFile(it, shareInsteadOfSave = true) },
                    onDelete = viewModel::deleteTransferFile,
                )
            } else {
            Column(modifier = Modifier.fillMaxSize()) {
                if (selecting) {
                    SelectionTopBar(
                        selectedCount = state.selectedIds.size,
                        allSelected = allSelected,
                        onClear = viewModel::clearSelection,
                        onToggleSelectAll = viewModel::selectAllVisible,
                    )
                } else {
                    TopGalleryBar(
                        title = state.activeChannel?.slug ?: "Home",
                        isChannel = state.activeChannel != null,
                        count = visibleImages.size,
                        sortLabel = state.sortOrder.label,
                        isSyncing = state.isSyncing,
                        onMenu = { scope.launch { drawerState.open() } },
                        onToggleSort = viewModel::toggleSortOrder,
                        onUpload = {
                            photoPicker.launch(
                                PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)
                            )
                        },
                        onSync = viewModel::syncNow,
                    )
                }

                if (visibleImages.isEmpty()) {
                    Box(
                        modifier = Modifier
                            .fillMaxSize()
                            .padding(32.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Column(horizontalAlignment = Alignment.CenterHorizontally) {
                            Text(
                                text = if (state.isSyncing) "Syncing with Google Drive..." else "No images here yet",
                                color = palette.fg,
                                fontSize = 15.sp,
                                fontWeight = FontWeight.SemiBold,
                            )
                            Spacer(Modifier.height(6.dp))
                            Text(
                                text = "Tap + to add photos or share from any app into koma.",
                                color = palette.muted,
                                fontSize = 13.sp,
                            )
                        }
                    }
                } else {
                    val showChannelTag = state.activeTab is ActiveTab.Home
                    val edgeZonePx = with(density) { 72.dp.toPx() }
                    val dragScrollStep = with(density) { 14.dp.toPx() }

                    LazyVerticalStaggeredGrid(
                        state = gridState,
                        columns = StaggeredGridCells.Adaptive(minSize = 114.dp),
                        contentPadding = PaddingValues(
                            start = 5.dp,
                            end = 5.dp,
                            top = 5.dp,
                            bottom = if (selecting) 96.dp else 28.dp,
                        ),
                        horizontalArrangement = Arrangement.spacedBy(4.dp),
                        verticalItemSpacing = 4.dp,
                        modifier = Modifier
                            .fillMaxSize()
                            .pointerInput(gridState) {
                                var anchorIdx: Int? = null
                                var lastIdx: Int? = null
                                detectDragGesturesAfterLongPress(
                                    onDragStart = { startOffset ->
                                        val hit = gridState.itemIndexAt(startOffset)
                                        if (hit != null && hit in latestImages.indices) {
                                            anchorIdx = hit
                                            lastIdx = hit
                                            haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                                            viewModel.addSelectedIds(listOf(latestImages[hit].id))
                                        }
                                    },
                                    onDragEnd = {
                                        anchorIdx = null
                                        lastIdx = null
                                        autoScrollSpeed = 0f
                                    },
                                    onDragCancel = {
                                        anchorIdx = null
                                        lastIdx = null
                                        autoScrollSpeed = 0f
                                    },
                                    onDrag = { change, _ ->
                                        val start = anchorIdx ?: return@detectDragGesturesAfterLongPress
                                        val pos = change.position
                                        val vpHeight = gridState.layoutInfo.viewportSize.height.toFloat()
                                        autoScrollSpeed = when {
                                            pos.y > vpHeight - edgeZonePx -> dragScrollStep
                                            pos.y < edgeZonePx -> -dragScrollStep
                                            else -> 0f
                                        }
                                        val hit = gridState.itemIndexAt(pos)
                                        if (hit != null && hit != lastIdx && hit in latestImages.indices) {
                                            lastIdx = hit
                                            val lo = minOf(start, hit)
                                            val hi = maxOf(start, hit)
                                            val ids = (lo..hi).map { latestImages[it].id }
                                            viewModel.addSelectedIds(ids)
                                        }
                                    },
                                )
                            },
                    ) {
                        itemsIndexed(
                            items = visibleImages,
                            key = { _, img -> img.id },
                            contentType = { _, _ -> "tile" },
                        ) { index, img ->
                            val isSelected = img.id in state.selectedIds
                            GalleryTile(
                                image = img,
                                imageLoader = viewModel.imageLoader,
                                isSelected = isSelected,
                                showChannelBadge = showChannelTag,
                                onTap = {
                                    if (selecting) {
                                        viewModel.toggleSelect(img.id)
                                    } else {
                                        viewModel.openViewer(index)
                                    }
                                },
                            )
                        }
                    }
                }
            }
            }

            AnimatedVisibility(
                visible = selecting,
                enter = slideInVertically(initialOffsetY = { it }) + fadeIn(),
                exit = slideOutVertically(targetOffsetY = { it }) + fadeOut(),
                modifier = Modifier
                    .align(Alignment.BottomCenter)
                    .navigationBarsPadding()
                    .padding(horizontal = 14.dp, vertical = 14.dp),
            ) {
                val selectedImages = remember(state.selectedIds, state.allImages) {
                    state.allImages.filter { it.id in state.selectedIds }
                }
                BatchActionDock(
                    count = selectedImages.size,
                    onShare = { viewModel.shareImages(selectedImages) },
                    onSave = { viewModel.saveImagesToGallery(selectedImages) },
                    onMove = { viewModel.requestMove(selectedImages.map { it.id }) },
                    onDelete = { viewModel.requestDelete(selectedImages.map { it.id }) },
                    onClear = viewModel::clearSelection,
                )
            }
        }
    }
}

private fun LazyStaggeredGridState.itemIndexAt(position: Offset): Int? {
    val px = position.x.toInt()
    val py = position.y.toInt()
    for (item in layoutInfo.visibleItemsInfo) {
        val left = item.offset.x
        val top = item.offset.y
        if (px in left..(left + item.size.width) && py in top..(top + item.size.height)) {
            return item.index
        }
    }
    return null
}

@Composable
private fun SelectionTopBar(
    selectedCount: Int,
    allSelected: Boolean,
    onClear: () -> Unit,
    onToggleSelectAll: () -> Unit,
) {
    val palette = LocalKomaPalette.current
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(palette.surface)
            .statusBarsPadding(),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .height(54.dp)
                .padding(horizontal = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onClear) {
                Icon(Icons.Rounded.Close, contentDescription = "Cancel selection", tint = palette.fg)
            }
            Text(
                text = "$selectedCount selected",
                color = palette.fg,
                fontSize = 16.sp,
                fontWeight = FontWeight.Bold,
                modifier = Modifier.weight(1f),
            )
            Surface(
                onClick = onToggleSelectAll,
                color = if (allSelected) palette.accent else palette.surfaceElevated,
                contentColor = if (allSelected) palette.accentFg else palette.fg,
                shape = RoundedCornerShape(7.dp),
                modifier = Modifier.padding(end = 8.dp),
            ) {
                Row(
                    modifier = Modifier.padding(horizontal = 10.dp, vertical = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(
                        Icons.Rounded.SelectAll,
                        contentDescription = null,
                        modifier = Modifier.size(16.dp),
                    )
                    Spacer(Modifier.width(5.dp))
                    Text(
                        text = if (allSelected) "Deselect all" else "Select all",
                        fontSize = 12.sp,
                        fontWeight = FontWeight.SemiBold,
                    )
                }
            }
        }
        HorizontalDivider(color = palette.border, thickness = 1.dp)
    }
}

@Composable
private fun TopGalleryBar(
    title: String,
    isChannel: Boolean,
    count: Int,
    sortLabel: String,
    isSyncing: Boolean,
    onMenu: () -> Unit,
    onToggleSort: () -> Unit,
    onUpload: () -> Unit,
    onSync: () -> Unit,
) {
    val palette = LocalKomaPalette.current
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(palette.surface)
            .statusBarsPadding(),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .height(54.dp)
                .padding(horizontal = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onMenu) {
                Icon(Icons.Rounded.Menu, contentDescription = "Channels", tint = palette.fg)
            }
            if (isChannel) {
                Text(
                    text = "#",
                    color = palette.muted,
                    fontSize = 17.sp,
                    fontWeight = FontWeight.Medium,
                    modifier = Modifier.padding(end = 2.dp),
                )
            }
            Text(
                text = title,
                color = palette.fg,
                fontSize = 17.sp,
                fontWeight = FontWeight.Bold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Spacer(Modifier.width(8.dp))
            Text(
                text = "$count",
                color = palette.muted,
                fontSize = 12.sp,
                fontFamily = FontFamily.Monospace,
            )
            Spacer(Modifier.weight(1f))
            Surface(
                onClick = onToggleSort,
                color = palette.surfaceElevated,
                shape = RoundedCornerShape(6.dp),
            ) {
                Row(
                    modifier = Modifier.padding(horizontal = 8.dp, vertical = 5.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(
                        Icons.Rounded.SwapVert,
                        contentDescription = "Sort order",
                        tint = palette.muted,
                        modifier = Modifier.size(15.dp),
                    )
                    Spacer(Modifier.width(3.dp))
                    Text(
                        text = sortLabel,
                        color = palette.fg,
                        fontSize = 11.5.sp,
                        fontWeight = FontWeight.Medium,
                    )
                }
            }
            IconButton(onClick = onUpload) {
                Icon(Icons.Rounded.Add, contentDescription = "Add images", tint = palette.fg)
            }
            IconButton(onClick = onSync, enabled = !isSyncing) {
                Icon(
                    Icons.Rounded.Refresh,
                    contentDescription = "Sync Drive",
                    tint = if (isSyncing) palette.syncBusy else palette.fg,
                )
            }
        }
        HorizontalDivider(color = palette.border, thickness = 1.dp)
    }
}

@Composable
private fun GalleryTile(
    image: DriveImage,
    imageLoader: ImageLoader,
    isSelected: Boolean,
    showChannelBadge: Boolean,
    onTap: () -> Unit,
) {
    val palette = LocalKomaPalette.current
    val context = LocalContext.current

    val thumbUrl = image.sizedThumbnailUrl(360)
        ?: "https://www.googleapis.com/drive/v3/files/${image.id}?alt=media"
    val request = remember(image.id, thumbUrl) {
        ImageRequest.Builder(context)
            .data(thumbUrl)
            .size(Size(360, 480))
            .precision(Precision.INEXACT)
            .allowRgb565(true)
            .memoryCacheKey("thumb_${image.id}")
            .diskCacheKey("thumb_${image.id}")
            .build()
    }

    Box(
        modifier = Modifier
            .fillMaxWidth()
            .aspectRatio(image.aspectRatio)
            .background(palette.surfaceElevated)
            .clickable(onClick = onTap),
    ) {
        AsyncImage(
            model = request,
            imageLoader = imageLoader,
            contentDescription = image.filename,
            contentScale = ContentScale.Crop,
            modifier = Modifier.fillMaxSize(),
        )

        if (isSelected) {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .border(4.dp, palette.selectionBezel)
                    .padding(4.dp)
                    .border(1.5.dp, palette.selectionRim),
            )
            Box(
                modifier = Modifier
                    .align(Alignment.TopEnd)
                    .padding(7.dp)
                    .size(22.dp)
                    .clip(RoundedCornerShape(4.dp))
                    .background(palette.accent),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    Icons.Rounded.Check,
                    contentDescription = "Selected",
                    tint = palette.accentFg,
                    modifier = Modifier.size(15.dp),
                )
            }
        }

        if (showChannelBadge) {
            Text(
                text = "#${image.channelSlug}",
                color = Color(0xFFF4F6FA),
                fontSize = 10.sp,
                fontFamily = FontFamily.Monospace,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier
                    .align(Alignment.TopStart)
                    .padding(5.dp)
                    .clip(RoundedCornerShape(3.dp))
                    .background(Color(0xC80A0C10))
                    .padding(horizontal = 5.dp, vertical = 2.dp),
            )
        }
    }
}

@Composable
private fun BatchActionDock(
    count: Int,
    onShare: () -> Unit,
    onSave: () -> Unit,
    onMove: () -> Unit,
    onDelete: () -> Unit,
    onClear: () -> Unit,
) {
    val palette = LocalKomaPalette.current
    Surface(
        color = palette.surfaceElevated,
        contentColor = palette.fg,
        shape = RoundedCornerShape(12.dp),
        shadowElevation = 10.dp,
        modifier = Modifier.border(1.dp, palette.border, RoundedCornerShape(12.dp)),
    ) {
        Row(
            modifier = Modifier.padding(horizontal = 10.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            Text(
                text = "$count",
                color = palette.accentFg,
                fontSize = 12.sp,
                fontWeight = FontWeight.Bold,
                fontFamily = FontFamily.Monospace,
                modifier = Modifier
                    .padding(start = 4.dp, end = 6.dp)
                    .clip(RoundedCornerShape(6.dp))
                    .background(palette.accent)
                    .padding(horizontal = 8.dp, vertical = 4.dp),
            )
            IconButton(onClick = onShare) {
                Icon(Icons.Rounded.Share, contentDescription = "Share", tint = palette.fg)
            }
            IconButton(onClick = onSave) {
                Icon(Icons.Rounded.Download, contentDescription = "Save to Gallery", tint = palette.fg)
            }
            IconButton(onClick = onMove) {
                Icon(Icons.AutoMirrored.Rounded.DriveFileMove, contentDescription = "Move", tint = palette.fg)
            }
            IconButton(onClick = onDelete) {
                Icon(Icons.Rounded.DeleteOutline, contentDescription = "Delete", tint = palette.danger)
            }
            IconButton(onClick = onClear) {
                Icon(Icons.Rounded.Close, contentDescription = "Clear selection", tint = palette.muted)
            }
        }
    }
}

@Composable
private fun ChannelDrawer(
    state: KomaUiState,
    onSelectTab: (ActiveTab) -> Unit,
    onNewChannel: () -> Unit,
    onToggleChannels: () -> Unit,
    onSelectTheme: (KomaTheme) -> Unit,
    onSignOut: () -> Unit,
) {
    val palette = LocalKomaPalette.current
    ModalDrawerSheet(
        drawerContainerColor = palette.surface,
        drawerContentColor = palette.fg,
        modifier = Modifier.width(284.dp),
    ) {
        Column(
            modifier = Modifier
                .fillMaxHeight()
                .statusBarsPadding()
                .navigationBarsPadding(),
        ) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp, vertical = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Image(
                    painter = painterResource(R.drawable.ic_koma_mascot),
                    contentDescription = "koma",
                    modifier = Modifier.size(26.dp),
                )
                Spacer(Modifier.width(10.dp))
                Text(
                    text = "koma",
                    color = palette.fg,
                    fontSize = 17.sp,
                    fontWeight = FontWeight.Bold,
                    modifier = Modifier.weight(1f),
                )
                Box(
                    modifier = Modifier
                        .size(7.dp)
                        .clip(CircleShape)
                        .background(if (state.isSyncing) palette.syncBusy else palette.syncOk),
                )
                Spacer(Modifier.width(5.dp))
                Text(
                    text = if (state.isSyncing) "Syncing" else "Synced",
                    color = palette.muted,
                    fontSize = 11.5.sp,
                )
            }

            val homeActive = state.activeTab is ActiveTab.Home
            DrawerNavItem(
                icon = Icons.Rounded.Home,
                label = "Home",
                count = state.allImages.size,
                active = homeActive,
                onClick = { onSelectTab(ActiveTab.Home) },
            )
            DrawerNavItem(
                icon = Icons.AutoMirrored.Rounded.Send,
                label = "Throwaway",
                count = state.throwawayFiles.size,
                active = state.activeTab is ActiveTab.Throwaway,
                onClick = { onSelectTab(ActiveTab.Throwaway) },
            )
            DrawerNavItem(
                icon = Icons.Rounded.FolderOpen,
                label = "Files",
                count = state.savedFiles.size,
                active = state.activeTab is ActiveTab.Files,
                onClick = { onSelectTab(ActiveTab.Files) },
            )

            Spacer(Modifier.height(10.dp))

            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(start = 18.dp, end = 8.dp, top = 4.dp, bottom = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Row(
                    modifier = Modifier
                        .weight(1f)
                        .clip(RoundedCornerShape(6.dp))
                        .clickable(onClick = onToggleChannels)
                        .padding(vertical = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        text = "CHANNELS",
                        color = palette.muted,
                        fontSize = 11.sp,
                        fontWeight = FontWeight.Bold,
                        fontFamily = FontFamily.Monospace,
                        letterSpacing = 1.sp,
                    )
                    Icon(
                        if (state.channelsCollapsed) Icons.Rounded.ChevronRight else Icons.Rounded.ExpandMore,
                        contentDescription = if (state.channelsCollapsed) "Show channels" else "Hide channels",
                        tint = palette.muted,
                        modifier = Modifier.size(16.dp),
                    )
                }
                IconButton(onClick = onNewChannel, modifier = Modifier.size(32.dp)) {
                    Icon(
                        Icons.Rounded.Add,
                        contentDescription = "New channel",
                        tint = palette.muted,
                        modifier = Modifier.size(18.dp),
                    )
                }
            }

            LazyColumn(
                modifier = Modifier.weight(1f),
                contentPadding = PaddingValues(horizontal = 10.dp, vertical = 4.dp),
            ) {
                if (!state.channelsCollapsed) items(state.channels, key = { it.id }) { ch ->
                    val selected = (state.activeTab as? ActiveTab.Channel)?.id == ch.id
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(vertical = 1.dp)
                            .clip(RoundedCornerShape(8.dp))
                            .background(if (selected) palette.accent.copy(alpha = 0.16f) else Color.Transparent)
                            .clickable { onSelectTab(ActiveTab.Channel(ch.id)) }
                            .padding(horizontal = 12.dp, vertical = 9.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            text = "#",
                            color = palette.muted,
                            fontSize = 14.sp,
                            modifier = Modifier.padding(end = 6.dp),
                        )
                        Text(
                            text = ch.slug,
                            color = palette.fg,
                            fontSize = 14.sp,
                            fontWeight = if (selected) FontWeight.SemiBold else FontWeight.Normal,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f),
                        )
                        Text(
                            text = "${ch.imageCount}",
                            color = palette.muted,
                            fontSize = 11.5.sp,
                            fontFamily = FontFamily.Monospace,
                        )
                    }
                }
            }

            HorizontalDivider(color = palette.border, thickness = 1.dp)

            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                for (t in KomaTheme.entries) {
                    val active = state.theme == t
                    Surface(
                        onClick = { onSelectTheme(t) },
                        color = if (active) palette.accent else palette.surfaceElevated,
                        contentColor = if (active) palette.accentFg else palette.fg,
                        shape = RoundedCornerShape(7.dp),
                        modifier = Modifier.weight(1f),
                    ) {
                        Box(
                            modifier = Modifier.padding(vertical = 7.dp),
                            contentAlignment = Alignment.Center,
                        ) {
                            Text(
                                text = t.label,
                                fontSize = 12.sp,
                                fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium,
                            )
                        }
                    }
                }
                IconButton(onClick = onSignOut) {
                    Icon(
                        Icons.AutoMirrored.Rounded.Logout,
                        contentDescription = "Sign out",
                        tint = palette.muted,
                        modifier = Modifier.size(19.dp),
                    )
                }
            }
        }
    }
}

@Composable
private fun DrawerNavItem(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    label: String,
    count: Int,
    active: Boolean,
    onClick: () -> Unit,
) {
    val palette = LocalKomaPalette.current
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 10.dp, vertical = 2.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(if (active) palette.accent.copy(alpha = 0.16f) else Color.Transparent)
            .clickable(onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(
            icon,
            contentDescription = null,
            tint = if (active) palette.fg else palette.muted,
            modifier = Modifier.size(18.dp),
        )
        Spacer(Modifier.width(10.dp))
        Text(
            text = label,
            color = palette.fg,
            fontSize = 14.sp,
            fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal,
            modifier = Modifier.weight(1f),
        )
        if (count > 0) {
            Text(
                text = "$count",
                color = palette.muted,
                fontSize = 11.5.sp,
                fontFamily = FontFamily.Monospace,
            )
        }
    }
}

@Composable
private fun FileBucketContent(
    bucket: FileBucket,
    files: List<DriveTransferFile>,
    isSyncing: Boolean,
    onMenu: () -> Unit,
    onUpload: () -> Unit,
    onSync: () -> Unit,
    onSave: (DriveTransferFile) -> Unit,
    onShare: (DriveTransferFile) -> Unit,
    onDelete: (DriveTransferFile) -> Unit,
) {
    val palette = LocalKomaPalette.current
    Column(modifier = Modifier.fillMaxSize()) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .background(palette.surface)
                .statusBarsPadding(),
        ) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .height(54.dp)
                    .padding(horizontal = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(onClick = onMenu) {
                    Icon(Icons.Rounded.Menu, contentDescription = "Menu", tint = palette.fg)
                }
                Text(
                    text = bucket.label,
                    color = palette.fg,
                    fontSize = 17.sp,
                    fontWeight = FontWeight.Bold,
                )
                Spacer(Modifier.width(8.dp))
                Text(
                    text = "${files.size}",
                    color = palette.muted,
                    fontSize = 12.sp,
                    fontFamily = FontFamily.Monospace,
                )
                Spacer(Modifier.weight(1f))
                IconButton(onClick = onUpload) {
                    Icon(Icons.Rounded.Add, contentDescription = "Upload files", tint = palette.fg)
                }
                IconButton(onClick = onSync, enabled = !isSyncing) {
                    Icon(
                        Icons.Rounded.Refresh,
                        contentDescription = "Refresh",
                        tint = if (isSyncing) palette.syncBusy else palette.fg,
                    )
                }
            }
            HorizontalDivider(color = palette.border, thickness = 1.dp)
        }

        Text(
            text = if (bucket == FileBucket.THROWAWAY) {
                "One-shot inbox: saving or sharing a file removes it from Drive automatically."
            } else {
                "Persistent work & general files synced with your PC."
            },
            color = palette.muted,
            fontSize = 12.sp,
            modifier = Modifier.padding(horizontal = 14.dp, vertical = 10.dp),
        )

        if (files.isEmpty()) {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(32.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    text = "Tap + to pick any file, or upload from PC.",
                    color = palette.muted,
                    fontSize = 13.5.sp,
                )
            }
        } else {
            LazyColumn(
                contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = 28.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.fillMaxSize(),
            ) {
                items(files, key = { it.id }) { file ->
                    Surface(
                        color = palette.surface,
                        contentColor = palette.fg,
                        shape = RoundedCornerShape(9.dp),
                        modifier = Modifier
                            .fillMaxWidth()
                            .border(1.dp, palette.border, RoundedCornerShape(9.dp)),
                    ) {
                        Row(
                            modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Box(
                                modifier = Modifier
                                    .clip(RoundedCornerShape(5.dp))
                                    .background(palette.accent.copy(alpha = 0.16f))
                                    .padding(horizontal = 8.dp, vertical = 5.dp),
                            ) {
                                Text(
                                    text = file.extBadge,
                                    color = palette.fg,
                                    fontSize = 10.5.sp,
                                    fontWeight = FontWeight.Bold,
                                    fontFamily = FontFamily.Monospace,
                                )
                            }
                            Spacer(Modifier.width(10.dp))
                            Column(modifier = Modifier.weight(1f)) {
                                Text(
                                    text = file.filename,
                                    color = palette.fg,
                                    fontSize = 13.5.sp,
                                    fontWeight = FontWeight.Medium,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                                Text(
                                    text = formatBytes(file.sizeBytes),
                                    color = palette.muted,
                                    fontSize = 11.5.sp,
                                    fontFamily = FontFamily.Monospace,
                                )
                            }
                            IconButton(onClick = { onSave(file) }) {
                                Icon(
                                    Icons.Rounded.Download,
                                    contentDescription = if (bucket == FileBucket.THROWAWAY) "Save & Clear" else "Save",
                                    tint = palette.fg,
                                )
                            }
                            IconButton(onClick = { onShare(file) }) {
                                Icon(
                                    Icons.Rounded.Share,
                                    contentDescription = "Share",
                                    tint = palette.fg,
                                )
                            }
                            IconButton(onClick = { onDelete(file) }) {
                                Icon(
                                    Icons.Rounded.DeleteOutline,
                                    contentDescription = "Delete",
                                    tint = palette.danger,
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}
