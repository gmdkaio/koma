package dev.koma.app.data

import androidx.compose.runtime.Immutable
import kotlinx.serialization.Serializable

@Immutable
@Serializable
data class DriveChannel(
    val id: String,
    val slug: String,
    val imageCount: Int = 0,
)

@Immutable
@Serializable
data class DriveImage(
    val id: String,
    val channelId: String,
    val channelSlug: String,
    val filename: String,
    val mimeType: String,
    val sizeBytes: Long = 0L,
    val width: Int = 0,
    val height: Int = 0,
    val createdAtIso: String = "",
    val createdAtMillis: Long = 0L,
    val thumbnailLink: String? = null,
) {
    val aspectRatio: Float
        get() = if (width > 0 && height > 0) {
            (width.toFloat() / height.toFloat()).coerceIn(0.48f, 2.2f)
        } else {
            1f
        }

    fun sizedThumbnailUrl(maxDim: Int = 480): String? {
        val raw = thumbnailLink ?: return null
        return raw.replace(Regex("=s\\d+$"), "=s$maxDim")
    }
}

@Serializable
enum class FileBucket(val folderName: String, val label: String) {
    THROWAWAY(".throwaway", "Throwaway"),
    FILES(".files", "Files"),
}

@Immutable
@Serializable
data class DriveTransferFile(
    val id: String,
    val bucket: FileBucket,
    val filename: String,
    val mimeType: String,
    val sizeBytes: Long = 0L,
    val createdAtMillis: Long = 0L,
) {
    val extBadge: String
        get() = filename.substringAfterLast('.', "")
            .uppercase()
            .takeIf { it.isNotEmpty() && it.length <= 5 }
            ?: "FILE"
}

@Serializable
data class CatalogCache(
    val rootFolderId: String,
    val channels: List<DriveChannel>,
    val images: List<DriveImage>,
    val throwawayFolderId: String? = null,
    val filesFolderId: String? = null,
    val transferFiles: List<DriveTransferFile> = emptyList(),
)

enum class KomaTheme(val key: String, val label: String) {
    OBSIDIAN("obsidian", "Obsidian"),
    GOFUN("paper", "Gofun");

    companion object {
        fun fromKey(raw: String?): KomaTheme = entries.firstOrNull { it.key == raw } ?: OBSIDIAN
    }
}

enum class SortOrder(val label: String) {
    NEWEST("Newest"),
    OLDEST("Oldest"),
}

sealed interface ActiveTab {
    data object Home : ActiveTab
    data object Throwaway : ActiveTab
    data object Files : ActiveTab
    data class Channel(val id: String) : ActiveTab
}

fun slugifyChannel(raw: String): String {
    val sb = StringBuilder()
    var prevUnderscore = false
    for (ch in raw.trim().lowercase()) {
        if (ch in 'a'..'z' || ch in '0'..'9') {
            sb.append(ch)
            prevUnderscore = false
        } else if (sb.isNotEmpty() && !prevUnderscore) {
            sb.append('_')
            prevUnderscore = true
        }
    }
    return sb.toString().trim('_')
}

fun nextChannelFilename(slug: String, existingNames: Collection<String>, ext: String): String {
    val cleanExt = ext.trimStart('.').lowercase().ifEmpty { "png" }
    val regex = Regex("^${Regex.escape(slug)}_(\\d+)\\.[a-z0-9]+$", RegexOption.IGNORE_CASE)
    var maxSeq = 0
    for (name in existingNames) {
        val m = regex.matchEntire(name)
        val n = m?.groupValues?.getOrNull(1)?.toIntOrNull() ?: 0
        if (n > maxSeq) maxSeq = n
    }
    val nextSeq = maxSeq + 1
    return "%s_%04d.%s".format(slug, nextSeq, cleanExt)
}

fun extFromMime(mimeType: String?, fallbackName: String? = null): String {
    val fromMime = when (mimeType?.lowercase()?.substringBefore(';')?.trim()) {
        "image/jpeg", "image/jpg" -> "jpg"
        "image/png" -> "png"
        "image/webp" -> "webp"
        "image/gif" -> "gif"
        "image/bmp" -> "bmp"
        else -> null
    }
    if (fromMime != null) return fromMime
    val ext = fallbackName?.substringAfterLast('.', "")?.lowercase().orEmpty()
    return when (ext) {
        "jpeg" -> "jpg"
        "jpg", "png", "webp", "gif", "bmp" -> ext
        else -> "png"
    }
}

fun formatBytes(bytes: Long): String {
    if (bytes < 1024L) return "$bytes B"
    val kb = bytes / 1024.0
    if (kb < 1024.0) return "%.0f KB".format(kb)
    return "%.1f MB".format(kb / 1024.0)
}
