package dev.koma.app.data

import android.content.ContentResolver
import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.ByteArrayOutputStream
import java.io.File
import java.time.Instant

class DriveRepository(
    private val context: Context,
    private val http: OkHttpClient,
    private val auth: AuthManager,
) {
    private val json = Json { ignoreUnknownKeys = true }
    private val cacheFile = File(context.filesDir, "catalog_cache.json")
    private val shareDir = File(context.cacheDir, "share").apply { mkdirs() }

    fun loadCachedCatalog(): CatalogCache? = runCatching {
        if (!cacheFile.exists()) return null
        json.decodeFromString<CatalogCache>(cacheFile.readText())
    }.getOrNull()

    fun saveCachedCatalog(catalog: CatalogCache) {
        runCatching {
            cacheFile.writeText(json.encodeToString(catalog))
        }
    }

    fun clearCache() {
        runCatching { cacheFile.delete() }
        runCatching { shareDir.listFiles()?.forEach { it.delete() } }
    }

    suspend fun syncCatalog(): CatalogCache = withContext(Dispatchers.IO) {
        val token = auth.getValidAccessToken()
        val rootId = findOrCreateRootFolder(token)

        val folderRows = listFiles(
            token = token,
            query = "mimeType = '$FOLDER_MIME' and '$rootId' in parents and trashed = false",
            fields = "nextPageToken,files(id,name)",
        )

        val channelMap = LinkedHashMap<String, String>()
        for (f in folderRows) {
            if (!f.name.startsWith(".")) {
                channelMap[f.id] = f.name
            }
        }

        val imageRows = if (channelMap.isEmpty()) {
            emptyList()
        } else {
            listFiles(
                token = token,
                query = "mimeType contains 'image/' and trashed = false",
                fields = "nextPageToken,files(id,name,mimeType,size,createdTime,parents,thumbnailLink,imageMediaMetadata(width,height))",
            )
        }

        val counts = HashMap<String, Int>()
        val images = ArrayList<DriveImage>(imageRows.size)
        for (row in imageRows) {
            val parentId = row.parents.firstOrNull { channelMap.containsKey(it) } ?: continue
            val slug = channelMap[parentId] ?: continue
            counts[parentId] = (counts[parentId] ?: 0) + 1
            images.add(row.toDriveImage(parentId, slug))
        }

        images.sortByDescending { it.createdAtMillis }

        val channels = channelMap.entries
            .map { (id, slug) -> DriveChannel(id = id, slug = slug, imageCount = counts[id] ?: 0) }
            .sortedWith(compareByDescending<DriveChannel> { it.imageCount }.thenBy { it.slug })

        val catalog = CatalogCache(
            rootFolderId = rootId,
            channels = channels,
            images = images,
        )
        saveCachedCatalog(catalog)
        catalog
    }

    suspend fun createChannel(rootFolderId: String, rawName: String): DriveChannel = withContext(Dispatchers.IO) {
        val slug = slugifyChannel(rawName)
        require(slug.isNotBlank()) { "Channel name must contain letters or numbers." }
        val token = auth.getValidAccessToken()

        val existing = listFiles(
            token = token,
            query = "mimeType = '$FOLDER_MIME' and name = '${escapeQuery(slug)}' and '$rootFolderId' in parents and trashed = false",
            fields = "files(id,name)",
        ).firstOrNull()

        if (existing != null) {
            return@withContext DriveChannel(id = existing.id, slug = existing.name, imageCount = 0)
        }

        val payload = json.encodeToString(
            CreateFolderBody(
                name = slug,
                mimeType = FOLDER_MIME,
                parents = listOf(rootFolderId),
            )
        )
        val req = Request.Builder()
            .url("https://www.googleapis.com/drive/v3/files?fields=id,name")
            .header("Authorization", "Bearer $token")
            .post(payload.toRequestBody(JSON_MEDIA_TYPE))
            .build()

        http.newCall(req).execute().use { resp ->
            val body = resp.body?.string().orEmpty()
            if (!resp.isSuccessful) error("Create channel failed (${resp.code}): $body")
            val created = json.decodeFromString<DriveFileDto>(body)
            DriveChannel(id = created.id, slug = created.name, imageCount = 0)
        }
    }

    suspend fun uploadImages(
        channel: DriveChannel,
        existingNames: List<String>,
        uris: List<Uri>,
        contentResolver: ContentResolver,
    ): List<DriveImage> = withContext(Dispatchers.IO) {
        val token = auth.getValidAccessToken()
        val runningNames = existingNames.toMutableList()
        // Pre-assign sequential filenames up front so up to 3 uploads can run in parallel.
        val planned = uris.map { uri ->
            val mime = contentResolver.getType(uri) ?: "image/png"
            val displayName = queryDisplayName(contentResolver, uri)
            val ext = extFromMime(mime, displayName)
            val filename = nextChannelFilename(channel.slug, runningNames, ext)
            runningNames.add(filename)
            Triple(uri, mime, filename)
        }
        val gate = Semaphore(3)
        coroutineScope {
            planned.map { (uri, mime, filename) ->
                async(Dispatchers.IO) {
                    gate.withPermit {
                        val bytes = contentResolver.openInputStream(uri)?.use { it.readBytes() }
                            ?: error("Could not read shared image")

                        val metadataJson = json.encodeToString(
                            UploadMetaBody(
                                name = filename,
                                parents = listOf(channel.id),
                            )
                        )
                        val boundary = "koma_${System.nanoTime()}"
                        val multipart = ByteArrayOutputStream().apply {
                            write("--$boundary\r\n".toByteArray())
                            write("Content-Type: application/json; charset=UTF-8\r\n\r\n".toByteArray())
                            write(metadataJson.toByteArray(Charsets.UTF_8))
                            write("\r\n--$boundary\r\n".toByteArray())
                            write("Content-Type: $mime\r\n\r\n".toByteArray())
                            write(bytes)
                            write("\r\n--$boundary--\r\n".toByteArray())
                        }.toByteArray()

                        val url = "https://www.googleapis.com/upload/drive/v3/files".toHttpUrl()
                            .newBuilder()
                            .addQueryParameter("uploadType", "multipart")
                            .addQueryParameter(
                                "fields",
                                "id,name,mimeType,size,createdTime,parents,thumbnailLink,imageMediaMetadata(width,height)"
                            )
                            .build()

                        val req = Request.Builder()
                            .url(url)
                            .header("Authorization", "Bearer $token")
                            .post(multipart.toRequestBody("multipart/related; boundary=$boundary".toMediaType()))
                            .build()

                        http.newCall(req).execute().use { resp ->
                            val raw = resp.body?.string().orEmpty()
                            if (!resp.isSuccessful) error("Upload failed (${resp.code}): $raw")
                            val dto = json.decodeFromString<DriveFileDto>(raw)
                            dto.toDriveImage(channel.id, channel.slug)
                        }
                    }
                }
            }.awaitAll()
        }
    }

    suspend fun moveImages(
        images: List<DriveImage>,
        targetChannel: DriveChannel,
        targetExistingNames: List<String>,
    ): List<DriveImage> = withContext(Dispatchers.IO) {
        val token = auth.getValidAccessToken()
        val runningNames = targetExistingNames.toMutableList()
        val moved = ArrayList<DriveImage>(images.size)

        for (img in images) {
            if (img.channelId == targetChannel.id) continue
            val ext = extFromMime(img.mimeType, img.filename)
            val newName = nextChannelFilename(targetChannel.slug, runningNames, ext)
            val url = "https://www.googleapis.com/drive/v3/files/${img.id}".toHttpUrl()
                .newBuilder()
                .addQueryParameter("addParents", targetChannel.id)
                .addQueryParameter("removeParents", img.channelId)
                .addQueryParameter(
                    "fields",
                    "id,name,mimeType,size,createdTime,parents,thumbnailLink,imageMediaMetadata(width,height)"
                )
                .build()

            val body = json.encodeToString(RenameBody(name = newName))
            val req = Request.Builder()
                .url(url)
                .header("Authorization", "Bearer $token")
                .patch(body.toRequestBody(JSON_MEDIA_TYPE))
                .build()

            http.newCall(req).execute().use { resp ->
                val raw = resp.body?.string().orEmpty()
                if (!resp.isSuccessful) error("Move failed (${resp.code}): $raw")
                val dto = json.decodeFromString<DriveFileDto>(raw)
                runningNames.add(dto.name)
                moved.add(dto.toDriveImage(targetChannel.id, targetChannel.slug))
            }
        }
        moved
    }

    suspend fun deleteImages(images: List<DriveImage>) = withContext(Dispatchers.IO) {
        val token = auth.getValidAccessToken()
        val body = """{"trashed":true}""".toRequestBody(JSON_MEDIA_TYPE)
        for (img in images) {
            val req = Request.Builder()
                .url("https://www.googleapis.com/drive/v3/files/${img.id}")
                .header("Authorization", "Bearer $token")
                .patch(body)
                .build()
            http.newCall(req).execute().use { resp ->
                if (!resp.isSuccessful && resp.code != 404) {
                    val raw = resp.body?.string().orEmpty()
                    error("Delete failed (${resp.code}): $raw")
                }
            }
        }
    }

    suspend fun downloadOriginal(image: DriveImage): File = withContext(Dispatchers.IO) {
        val safeName = "${image.id}_${image.filename.replace(Regex("[^a-zA-Z0-9._-]"), "_")}"
        val dest = File(shareDir, safeName)
        if (dest.exists() && dest.length() > 0L) {
            return@withContext dest
        }

        val token = auth.getValidAccessToken()
        val req = Request.Builder()
            .url("https://www.googleapis.com/drive/v3/files/${image.id}?alt=media")
            .header("Authorization", "Bearer $token")
            .get()
            .build()

        http.newCall(req).execute().use { resp ->
            if (!resp.isSuccessful) {
                error("Download failed (${resp.code})")
            }
            val tmp = File(shareDir, "$safeName.tmp")
            resp.body?.byteStream()?.use { input ->
                tmp.outputStream().use { output -> input.copyTo(output) }
            } ?: error("Empty download response")
            if (!tmp.renameTo(dest)) {
                tmp.copyTo(dest, overwrite = true)
                tmp.delete()
            }
            dest
        }
    }

    private fun findOrCreateRootFolder(token: String): String {
        val existing = listFiles(
            token = token,
            query = "mimeType = '$FOLDER_MIME' and name = 'koma' and 'root' in parents and trashed = false",
            fields = "files(id,name)",
        ).firstOrNull()
        if (existing != null) return existing.id

        val payload = json.encodeToString(
            CreateFolderBody(name = "koma", mimeType = FOLDER_MIME, parents = listOf("root"))
        )
        val req = Request.Builder()
            .url("https://www.googleapis.com/drive/v3/files?fields=id,name")
            .header("Authorization", "Bearer $token")
            .post(payload.toRequestBody(JSON_MEDIA_TYPE))
            .build()

        return http.newCall(req).execute().use { resp ->
            val raw = resp.body?.string().orEmpty()
            if (!resp.isSuccessful) error("Could not create koma folder (${resp.code}): $raw")
            json.decodeFromString<DriveFileDto>(raw).id
        }
    }

    private fun listFiles(token: String, query: String, fields: String): List<DriveFileDto> {
        val out = ArrayList<DriveFileDto>()
        var pageToken: String? = null
        do {
            val urlBuilder = "https://www.googleapis.com/drive/v3/files".toHttpUrl()
                .newBuilder()
                .addQueryParameter("q", query)
                .addQueryParameter("fields", fields)
                .addQueryParameter("pageSize", "1000")
                .addQueryParameter("spaces", "drive")
            if (!pageToken.isNullOrBlank()) {
                urlBuilder.addQueryParameter("pageToken", pageToken)
            }
            val req = Request.Builder()
                .url(urlBuilder.build())
                .header("Authorization", "Bearer $token")
                .get()
                .build()
            http.newCall(req).execute().use { resp ->
                val raw = resp.body?.string().orEmpty()
                if (!resp.isSuccessful) error("Drive list failed (${resp.code}): $raw")
                val page = json.decodeFromString<DriveListResponse>(raw)
                out.addAll(page.files)
                pageToken = page.nextPageToken
            }
        } while (!pageToken.isNullOrBlank())
        return out
    }

    private fun queryDisplayName(resolver: ContentResolver, uri: Uri): String? = runCatching {
        resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) cursor.getString(0) else null
        }
    }.getOrNull()

    private fun escapeQuery(value: String): String = value.replace("'", "\\'")

    @Serializable
    private data class DriveListResponse(
        val files: List<DriveFileDto> = emptyList(),
        val nextPageToken: String? = null,
    )

    @Serializable
    private data class DriveFileDto(
        val id: String,
        val name: String = "",
        val mimeType: String = "image/png",
        val size: String? = null,
        val createdTime: String? = null,
        val parents: List<String> = emptyList(),
        val thumbnailLink: String? = null,
        val imageMediaMetadata: ImageMetaDto? = null,
    ) {
        fun toDriveImage(channelId: String, channelSlug: String): DriveImage {
            val millis = runCatching {
                if (!createdTime.isNullOrBlank()) Instant.parse(createdTime).toEpochMilli() else 0L
            }.getOrDefault(0L)
            return DriveImage(
                id = id,
                channelId = channelId,
                channelSlug = channelSlug,
                filename = name,
                mimeType = mimeType,
                sizeBytes = size?.toLongOrNull() ?: 0L,
                width = imageMediaMetadata?.width ?: 0,
                height = imageMediaMetadata?.height ?: 0,
                createdAtIso = createdTime.orEmpty(),
                createdAtMillis = millis,
                thumbnailLink = thumbnailLink,
            )
        }
    }

    @Serializable
    private data class ImageMetaDto(
        val width: Int = 0,
        val height: Int = 0,
    )

    @Serializable
    private data class CreateFolderBody(
        val name: String,
        val mimeType: String,
        val parents: List<String>,
    )

    @Serializable
    private data class UploadMetaBody(
        val name: String,
        val parents: List<String>,
    )

    @Serializable
    private data class RenameBody(
        val name: String,
    )

    companion object {
        private const val FOLDER_MIME = "application/vnd.google-apps.folder"
        private val JSON_MEDIA_TYPE = "application/json; charset=utf-8".toMediaType()
    }
}
