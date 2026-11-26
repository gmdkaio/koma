package dev.koma.app.ui

import android.app.Application
import android.content.Intent
import android.content.IntentSender
import android.net.Uri
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import coil3.ImageLoader
import coil3.disk.DiskCache
import coil3.disk.directory
import coil3.memory.MemoryCache
import coil3.network.okhttp.OkHttpNetworkFetcherFactory
import dev.koma.app.data.ActiveTab
import dev.koma.app.data.AuthManager
import dev.koma.app.data.CatalogCache
import dev.koma.app.data.DriveChannel
import dev.koma.app.data.DriveImage
import dev.koma.app.data.DriveRepository
import dev.koma.app.data.DriveTransferFile
import dev.koma.app.data.FileBucket
import dev.koma.app.data.KomaTheme
import dev.koma.app.data.MediaExport
import dev.koma.app.data.SignInOutcome
import dev.koma.app.data.SortOrder
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import okhttp3.OkHttpClient
import java.io.File
import java.util.concurrent.TimeUnit

data class KomaUiState(
    val isSignedIn: Boolean = false,
    val isSyncing: Boolean = false,
    val pendingAuthResolution: IntentSender? = null,
    val busyMessage: String? = null,
    val toastMessage: String? = null,
    val theme: KomaTheme = KomaTheme.OBSIDIAN,
    val channelsCollapsed: Boolean = false,
    val activeTab: ActiveTab = ActiveTab.Home,
    val sortOrder: SortOrder = SortOrder.NEWEST,
    val rootFolderId: String? = null,
    val throwawayFolderId: String? = null,
    val filesFolderId: String? = null,
    val channels: List<DriveChannel> = emptyList(),
    val allImages: List<DriveImage> = emptyList(),
    val transferFiles: List<DriveTransferFile> = emptyList(),
    val selectedIds: Set<String> = emptySet(),
    val viewerIndex: Int? = null,
    val pendingShareUris: List<Uri> = emptyList(),
    val pendingMoveIds: List<String> = emptyList(),
    val pendingDeleteIds: List<String> = emptyList(),
    val showCreateChannel: Boolean = false,
) {
    val throwawayFiles: List<DriveTransferFile>
        get() = transferFiles.filter { it.bucket == FileBucket.THROWAWAY }

    val savedFiles: List<DriveTransferFile>
        get() = transferFiles.filter { it.bucket == FileBucket.FILES }

    val visibleImages: List<DriveImage>
        get() {
            val filtered = when (activeTab) {
                ActiveTab.Home -> allImages
                ActiveTab.Throwaway, ActiveTab.Files -> emptyList()
                is ActiveTab.Channel -> allImages.filter { it.channelId == activeTab.id }
            }
            return when (sortOrder) {
                SortOrder.NEWEST -> filtered.sortedByDescending { it.createdAtMillis }
                SortOrder.OLDEST -> filtered.sortedBy { it.createdAtMillis }
            }
        }

    val activeChannel: DriveChannel?
        get() = (activeTab as? ActiveTab.Channel)?.let { tab ->
            channels.firstOrNull { it.id == tab.id }
        }
}

class KomaViewModel(application: Application) : AndroidViewModel(application) {
    private val baseHttp = OkHttpClient.Builder()
        .connectTimeout(20, TimeUnit.SECONDS)
        .readTimeout(45, TimeUnit.SECONDS)
        .writeTimeout(60, TimeUnit.SECONDS)
        .build()

    val auth = AuthManager(application)
    private val repo = DriveRepository(application, baseHttp, auth)

    private val coilHttp: OkHttpClient = baseHttp.newBuilder()
        .addInterceptor { chain ->
            val req = chain.request()
            val host = req.url.host
            val needsAuth = host.endsWith("googleapis.com") || host.endsWith("googleusercontent.com")
            if (!needsAuth) {
                return@addInterceptor chain.proceed(req)
            }
            val token = auth.currentAccessTokenBlocking()
                ?: runCatching { runBlocking { auth.getValidAccessToken() } }.getOrNull()
            val nextReq = if (!token.isNullOrBlank()) {
                req.newBuilder().header("Authorization", "Bearer $token").build()
            } else {
                req
            }
            chain.proceed(nextReq)
        }
        .build()

    val imageLoader: ImageLoader = ImageLoader.Builder(application)
        .components {
            add(OkHttpNetworkFetcherFactory(callFactory = { coilHttp }))
        }
        .memoryCache {
            MemoryCache.Builder()
                .maxSizePercent(application, 0.25)
                .build()
        }
        .diskCache {
            DiskCache.Builder()
                .directory(File(application.cacheDir, "coil_thumbs"))
                .maxSizeBytes(150L * 1024L * 1024L)
                .build()
        }
        .build()

    private val _uiState = MutableStateFlow(
        KomaUiState(
            isSignedIn = auth.isSignedIn,
            theme = auth.loadTheme(),
            channelsCollapsed = auth.loadChannelsCollapsed(),
        )
    )
    val uiState: StateFlow<KomaUiState> = _uiState.asStateFlow()

    init {
        if (auth.isSignedIn) {
            repo.loadCachedCatalog()?.let { cached ->
                applyCatalog(cached)
            }
            syncNow()
        }
    }

    fun startSignIn() {
        viewModelScope.launch {
            _uiState.update { it.copy(busyMessage = "Connecting to Google Drive...") }
            val result = auth.requestSignIn()
            _uiState.update { it.copy(busyMessage = null) }
            result.onSuccess { outcome ->
                when (outcome) {
                    SignInOutcome.SignedIn -> {
                        _uiState.update { it.copy(isSignedIn = true) }
                        toast("Connected to Google Drive")
                        syncNow()
                    }
                    is SignInOutcome.NeedsResolution -> {
                        _uiState.update { it.copy(pendingAuthResolution = outcome.intentSender) }
                    }
                }
            }.onFailure { err ->
                toast("Sign-in failed: ${err.message}")
            }
        }
    }

    fun clearPendingAuthResolution() {
        _uiState.update { it.copy(pendingAuthResolution = null) }
    }

    fun handleAuthActivityResult(resultCode: Int, data: Intent?) {
        _uiState.update { it.copy(pendingAuthResolution = null) }
        if (resultCode != android.app.Activity.RESULT_OK) {
            toast("Google sign-in canceled")
            return
        }
        auth.handleAuthorizationResult(data)
            .onSuccess {
                _uiState.update { it.copy(isSignedIn = true) }
                toast("Connected to Google Drive")
                syncNow()
            }
            .onFailure { err ->
                toast(err.message ?: "Google sign-in failed")
            }
    }

    fun handleIncomingShare(uris: List<Uri>) {
        if (uris.isEmpty()) return
        _uiState.update { it.copy(pendingShareUris = uris) }
        if (auth.isSignedIn && _uiState.value.channels.isEmpty()) {
            syncNow()
        }
    }

    fun dismissIncomingShare() {
        _uiState.update { it.copy(pendingShareUris = emptyList()) }
    }

    fun syncNow() {
        if (!auth.isSignedIn || _uiState.value.isSyncing) return
        viewModelScope.launch {
            _uiState.update { it.copy(isSyncing = true) }
            runCatching { repo.syncCatalog() }
                .onSuccess { catalog ->
                    applyCatalog(catalog)
                    _uiState.update { it.copy(isSyncing = false) }
                }
                .onFailure { err ->
                    val stillSignedIn = auth.isSignedIn
                    _uiState.update {
                        it.copy(
                            isSyncing = false,
                            isSignedIn = stillSignedIn,
                        )
                    }
                    toast(err.message ?: "Sync failed")
                }
        }
    }

    fun setTheme(theme: KomaTheme) {
        auth.saveTheme(theme)
        _uiState.update { it.copy(theme = theme) }
    }

    fun toggleChannelsCollapsed() {
        val collapsed = !_uiState.value.channelsCollapsed
        auth.saveChannelsCollapsed(collapsed)
        _uiState.update { it.copy(channelsCollapsed = collapsed) }
    }

    fun selectTab(tab: ActiveTab) {
        _uiState.update {
            it.copy(
                activeTab = tab,
                selectedIds = emptySet(),
                viewerIndex = null,
            )
        }
    }

    fun toggleSortOrder() {
        _uiState.update {
            val next = if (it.sortOrder == SortOrder.NEWEST) SortOrder.OLDEST else SortOrder.NEWEST
            it.copy(sortOrder = next)
        }
    }

    fun toggleSelect(imageId: String) {
        _uiState.update { state ->
            val next = state.selectedIds.toMutableSet()
            if (!next.add(imageId)) next.remove(imageId)
            state.copy(selectedIds = next)
        }
    }

    fun addSelectedIds(ids: Collection<String>) {
        if (ids.isEmpty()) return
        _uiState.update { state ->
            val next = state.selectedIds + ids
            if (next.size == state.selectedIds.size) state else state.copy(selectedIds = next)
        }
    }

    fun selectAllVisible() {
        _uiState.update { state ->
            val allVisible = state.visibleImages.map { it.id }.toSet()
            val next = if (state.selectedIds.size == allVisible.size && allVisible.isNotEmpty()) {
                emptySet()
            } else {
                allVisible
            }
            state.copy(selectedIds = next)
        }
    }

    fun clearSelection() {
        _uiState.update { it.copy(selectedIds = emptySet()) }
    }

    fun openViewer(index: Int) {
        _uiState.update { it.copy(viewerIndex = index) }
    }

    fun closeViewer() {
        _uiState.update { it.copy(viewerIndex = null) }
    }

    fun setShowCreateChannel(show: Boolean) {
        _uiState.update { it.copy(showCreateChannel = show) }
    }

    fun createChannel(rawName: String, onCreated: ((DriveChannel) -> Unit)? = null) {
        viewModelScope.launch {
            _uiState.update { it.copy(showCreateChannel = false, busyMessage = "Creating channel...") }
            runCatching {
                val rootId = _uiState.value.rootFolderId ?: repo.syncCatalog().rootFolderId
                val ch = repo.createChannel(rootId, rawName)
                val updatedChannels = (_uiState.value.channels.filterNot { it.id == ch.id } + ch)
                    .sortedWith(compareByDescending<DriveChannel> { it.imageCount }.thenBy { it.slug })
                val nextCatalog = CatalogCache(
                    rootFolderId = rootId,
                    channels = updatedChannels,
                    images = _uiState.value.allImages,
                )
                repo.saveCachedCatalog(nextCatalog)
                Pair(nextCatalog, ch)
            }.onSuccess { (catalog, ch) ->
                applyCatalog(catalog)
                _uiState.update { it.copy(busyMessage = null) }
                toast("Created #${ch.slug}")
                onCreated?.invoke(ch)
            }.onFailure { err ->
                _uiState.update { it.copy(busyMessage = null) }
                toast(err.message ?: "Create channel failed")
            }
        }
    }

    fun uploadUrisToChannel(channel: DriveChannel, uris: List<Uri>) {
        if (uris.isEmpty()) return
        viewModelScope.launch {
            _uiState.update {
                it.copy(
                    pendingShareUris = emptyList(),
                    busyMessage = "Uploading ${uris.size} image(s) to #${channel.slug}...",
                )
            }
            val existingNames = _uiState.value.allImages
                .filter { it.channelId == channel.id }
                .map { it.filename }
            runCatching {
                repo.uploadImages(
                    channel = channel,
                    existingNames = existingNames,
                    uris = uris,
                    contentResolver = getApplication<Application>().contentResolver,
                )
            }.onSuccess { added ->
                val mergedImages = (added + _uiState.value.allImages)
                    .distinctBy { it.id }
                    .sortedByDescending { it.createdAtMillis }
                updateLocalCatalog(mergedImages)
                _uiState.update { it.copy(busyMessage = null) }
                toast("Uploaded ${added.size} image(s) to #${channel.slug}")
            }.onFailure { err ->
                _uiState.update { it.copy(busyMessage = null) }
                toast(err.message ?: "Upload failed")
            }
        }
    }

    fun requestMove(ids: List<String>) {
        if (ids.isEmpty()) return
        _uiState.update { it.copy(pendingMoveIds = ids) }
    }

    fun dismissMove() {
        _uiState.update { it.copy(pendingMoveIds = emptyList()) }
    }

    fun confirmMoveTo(targetChannel: DriveChannel) {
        val ids = _uiState.value.pendingMoveIds
        if (ids.isEmpty()) return
        val targets = _uiState.value.allImages.filter { it.id in ids }
        viewModelScope.launch {
            _uiState.update {
                it.copy(
                    pendingMoveIds = emptyList(),
                    busyMessage = "Moving ${targets.size} image(s)...",
                )
            }
            val existingNames = _uiState.value.allImages
                .filter { it.channelId == targetChannel.id }
                .map { it.filename }
            runCatching {
                repo.moveImages(targets, targetChannel, existingNames)
            }.onSuccess { moved ->
                val movedMap = moved.associateBy { it.id }
                val nextImages = _uiState.value.allImages.map { movedMap[it.id] ?: it }
                updateLocalCatalog(nextImages)
                _uiState.update {
                    it.copy(
                        busyMessage = null,
                        selectedIds = emptySet(),
                        viewerIndex = null,
                    )
                }
                toast("Moved ${moved.size} image(s) to #${targetChannel.slug}")
            }.onFailure { err ->
                _uiState.update { it.copy(busyMessage = null) }
                toast(err.message ?: "Move failed")
            }
        }
    }

    fun requestDelete(ids: List<String>) {
        if (ids.isEmpty()) return
        _uiState.update { it.copy(pendingDeleteIds = ids) }
    }

    fun dismissDelete() {
        _uiState.update { it.copy(pendingDeleteIds = emptyList()) }
    }

    fun confirmDelete() {
        val ids = _uiState.value.pendingDeleteIds
        if (ids.isEmpty()) return
        val targets = _uiState.value.allImages.filter { it.id in ids }
        viewModelScope.launch {
            _uiState.update {
                it.copy(
                    pendingDeleteIds = emptyList(),
                    busyMessage = "Deleting ${targets.size} image(s)...",
                )
            }
            runCatching {
                repo.deleteImages(targets)
            }.onSuccess {
                val idSet = ids.toSet()
                val remaining = _uiState.value.allImages.filterNot { it.id in idSet }
                updateLocalCatalog(remaining)
                _uiState.update { state ->
                    val nextViewer = state.viewerIndex?.let { idx ->
                        val nextCount = state.visibleImages.size
                        if (nextCount == 0) null else idx.coerceAtMost(nextCount - 1)
                    }
                    state.copy(
                        busyMessage = null,
                        selectedIds = emptySet(),
                        viewerIndex = nextViewer,
                    )
                }
                toast("Deleted ${targets.size} image(s)")
            }.onFailure { err ->
                _uiState.update { it.copy(busyMessage = null) }
                toast(err.message ?: "Delete failed")
            }
        }
    }

    fun shareImages(images: List<DriveImage>) {
        if (images.isEmpty()) return
        viewModelScope.launch {
            _uiState.update { it.copy(busyMessage = "Preparing ${images.size} image(s) to share...") }
            runCatching {
                images.map { img -> img to repo.downloadOriginal(img) }
            }.onSuccess { files ->
                _uiState.update { it.copy(busyMessage = null, selectedIds = emptySet()) }
                MediaExport.shareImages(getApplication(), files)
            }.onFailure { err ->
                _uiState.update { it.copy(busyMessage = null) }
                toast(err.message ?: "Share failed")
            }
        }
    }

    fun saveImagesToGallery(images: List<DriveImage>) {
        if (images.isEmpty()) return
        viewModelScope.launch {
            _uiState.update { it.copy(busyMessage = "Saving ${images.size} image(s) to Gallery...") }
            runCatching {
                val files = images.map { img -> img to repo.downloadOriginal(img) }
                MediaExport.saveImagesToGallery(getApplication(), files)
            }.onSuccess { savedCount ->
                _uiState.update { it.copy(busyMessage = null, selectedIds = emptySet()) }
                toast("Saved $savedCount image(s) to Pictures/koma")
            }.onFailure { err ->
                _uiState.update { it.copy(busyMessage = null) }
                toast(err.message ?: "Save failed")
            }
        }
    }

    fun uploadToBucket(bucket: FileBucket, uris: List<Uri>) {
        if (uris.isEmpty()) return
        viewModelScope.launch {
            _uiState.update {
                it.copy(
                    pendingShareUris = emptyList(),
                    busyMessage = "Uploading ${uris.size} file(s) to ${bucket.label}...",
                )
            }
            runCatching {
                val rootId = _uiState.value.rootFolderId ?: repo.syncCatalog().rootFolderId
                val existingFolderId = when (bucket) {
                    FileBucket.THROWAWAY -> _uiState.value.throwawayFolderId
                    FileBucket.FILES -> _uiState.value.filesFolderId
                }
                val folderId = existingFolderId ?: repo.ensureSpecialFolder(rootId, bucket)
                val added = repo.uploadTransferFiles(
                    folderId = folderId,
                    bucket = bucket,
                    uris = uris,
                    contentResolver = getApplication<Application>().contentResolver,
                )
                Triple(rootId, folderId, added)
            }.onSuccess { (rootId, folderId, added) ->
                val merged = (added + _uiState.value.transferFiles)
                    .distinctBy { it.id }
                    .sortedByDescending { it.createdAtMillis }
                _uiState.update { st ->
                    val nextThrowId = if (bucket == FileBucket.THROWAWAY) folderId else st.throwawayFolderId
                    val nextFilesId = if (bucket == FileBucket.FILES) folderId else st.filesFolderId
                    repo.saveCachedCatalog(
                        CatalogCache(
                            rootFolderId = rootId,
                            channels = st.channels,
                            images = st.allImages,
                            throwawayFolderId = nextThrowId,
                            filesFolderId = nextFilesId,
                            transferFiles = merged,
                        )
                    )
                    st.copy(
                        rootFolderId = rootId,
                        throwawayFolderId = nextThrowId,
                        filesFolderId = nextFilesId,
                        transferFiles = merged,
                        busyMessage = null,
                    )
                }
                toast("Uploaded ${added.size} file(s) to ${bucket.label}")
            }.onFailure { err ->
                _uiState.update { it.copy(busyMessage = null) }
                toast(err.message ?: "Upload failed")
            }
        }
    }

    fun receiveTransferFile(file: DriveTransferFile, shareInsteadOfSave: Boolean) {
        val autoDelete = file.bucket == FileBucket.THROWAWAY
        viewModelScope.launch {
            _uiState.update { it.copy(busyMessage = "Downloading ${file.filename}...") }
            runCatching {
                val local = repo.downloadTransferFile(file)
                if (shareInsteadOfSave) {
                    MediaExport.shareTransferFile(getApplication(), file, local)
                } else {
                    val saved = MediaExport.saveFileToDownloads(getApplication(), file, local)
                    if (!saved) error("Could not save to Downloads/koma")
                }
                if (autoDelete) {
                    repo.deleteTransferFile(file.id)
                }
            }.onSuccess {
                if (autoDelete) {
                    removeLocalTransferFile(file.id)
                    toast(
                        if (shareInsteadOfSave) "Shared & cleared from Throwaway"
                        else "Saved to Downloads/koma & cleared from Throwaway"
                    )
                } else if (!shareInsteadOfSave) {
                    toast("Saved ${file.filename} to Downloads/koma")
                }
                _uiState.update { it.copy(busyMessage = null) }
            }.onFailure { err ->
                _uiState.update { it.copy(busyMessage = null) }
                toast(err.message ?: "Download failed")
            }
        }
    }

    fun refreshTransfersOnly() {
        val rootId = _uiState.value.rootFolderId ?: return
        if (!auth.isSignedIn || _uiState.value.isSyncing) return
        viewModelScope.launch {
            runCatching { repo.syncTransfersOnly(rootId) }
                .onSuccess { (throwId, filesId, transfers) ->
                    _uiState.update { st ->
                        repo.saveCachedCatalog(
                            CatalogCache(
                                rootFolderId = rootId,
                                channels = st.channels,
                                images = st.allImages,
                                throwawayFolderId = throwId,
                                filesFolderId = filesId,
                                transferFiles = transfers,
                            )
                        )
                        st.copy(
                            throwawayFolderId = throwId,
                            filesFolderId = filesId,
                            transferFiles = transfers,
                        )
                    }
                }
        }
    }

    fun deleteTransferFile(file: DriveTransferFile) {
        viewModelScope.launch {
            _uiState.update { it.copy(busyMessage = "Deleting ${file.filename}...") }
            runCatching { repo.deleteTransferFile(file.id) }
                .onSuccess {
                    removeLocalTransferFile(file.id)
                    _uiState.update { it.copy(busyMessage = null) }
                    toast("Deleted ${file.filename}")
                }
                .onFailure { err ->
                    _uiState.update { it.copy(busyMessage = null) }
                    toast(err.message ?: "Delete failed")
                }
        }
    }

    private fun removeLocalTransferFile(fileId: String) {
        _uiState.update { st ->
            val remaining = st.transferFiles.filterNot { it.id == fileId }
            st.rootFolderId?.let { rootId ->
                repo.saveCachedCatalog(
                    CatalogCache(
                        rootFolderId = rootId,
                        channels = st.channels,
                        images = st.allImages,
                        throwawayFolderId = st.throwawayFolderId,
                        filesFolderId = st.filesFolderId,
                        transferFiles = remaining,
                    )
                )
            }
            st.copy(transferFiles = remaining)
        }
    }

    fun signOut() {
        auth.signOut()
        repo.clearCache()
        _uiState.update {
            it.copy(
                isSignedIn = false,
                isSyncing = false,
                rootFolderId = null,
                throwawayFolderId = null,
                filesFolderId = null,
                channels = emptyList(),
                allImages = emptyList(),
                transferFiles = emptyList(),
                selectedIds = emptySet(),
                viewerIndex = null,
                activeTab = ActiveTab.Home,
            )
        }
        toast("Signed out")
    }

    fun clearToast() {
        _uiState.update { it.copy(toastMessage = null) }
    }

    private fun toast(msg: String) {
        _uiState.update { it.copy(toastMessage = msg) }
    }

    private fun applyCatalog(catalog: CatalogCache) {
        _uiState.update { state ->
            val validTab = when (val tab = state.activeTab) {
                ActiveTab.Home, ActiveTab.Throwaway, ActiveTab.Files -> tab
                is ActiveTab.Channel -> if (catalog.channels.any { it.id == tab.id }) tab else ActiveTab.Home
            }
            state.copy(
                rootFolderId = catalog.rootFolderId,
                throwawayFolderId = catalog.throwawayFolderId,
                filesFolderId = catalog.filesFolderId,
                channels = catalog.channels,
                allImages = catalog.images,
                transferFiles = catalog.transferFiles,
                activeTab = validTab,
            )
        }
    }

    private fun updateLocalCatalog(nextImages: List<DriveImage>) {
        val state = _uiState.value
        val counts = nextImages.groupingBy { it.channelId }.eachCount()
        val nextChannels = state.channels
            .map { it.copy(imageCount = counts[it.id] ?: 0) }
            .sortedWith(compareByDescending<DriveChannel> { it.imageCount }.thenBy { it.slug })
        val rootId = state.rootFolderId
        if (rootId != null) {
            repo.saveCachedCatalog(
                CatalogCache(
                    rootFolderId = rootId,
                    channels = nextChannels,
                    images = nextImages,
                    throwawayFolderId = state.throwawayFolderId,
                    filesFolderId = state.filesFolderId,
                    transferFiles = state.transferFiles,
                )
            )
        }
        _uiState.update {
            it.copy(
                channels = nextChannels,
                allImages = nextImages,
            )
        }
    }
}
