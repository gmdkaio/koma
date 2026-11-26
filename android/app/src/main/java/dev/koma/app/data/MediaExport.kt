package dev.koma.app.data

import android.content.ClipData
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import androidx.core.content.FileProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

object MediaExport {
    suspend fun saveImagesToGallery(
        context: Context,
        items: List<Pair<DriveImage, File>>,
    ): Int = withContext(Dispatchers.IO) {
        val resolver = context.contentResolver
        var saved = 0
        for ((image, file) in items) {
            val values = ContentValues().apply {
                put(MediaStore.Images.Media.DISPLAY_NAME, image.filename)
                put(MediaStore.Images.Media.MIME_TYPE, image.mimeType.ifBlank { "image/png" })
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    put(
                        MediaStore.Images.Media.RELATIVE_PATH,
                        "${Environment.DIRECTORY_PICTURES}/koma"
                    )
                    put(MediaStore.Images.Media.IS_PENDING, 1)
                }
            }
            val uri = resolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values)
                ?: continue
            try {
                resolver.openOutputStream(uri)?.use { out ->
                    file.inputStream().use { input -> input.copyTo(out) }
                }
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    val done = ContentValues().apply {
                        put(MediaStore.Images.Media.IS_PENDING, 0)
                    }
                    resolver.update(uri, done, null, null)
                }
                saved++
            } catch (_: Throwable) {
                resolver.delete(uri, null, null)
            }
        }
        saved
    }

    fun shareImages(context: Context, items: List<Pair<DriveImage, File>>) {
        if (items.isEmpty()) return
        val authority = "${context.packageName}.fileprovider"
        val uris = ArrayList<Uri>(items.size)
        for ((_, file) in items) {
            uris.add(FileProvider.getUriForFile(context, authority, file))
        }

        val intent = if (uris.size == 1) {
            Intent(Intent.ACTION_SEND).apply {
                type = items.first().first.mimeType.ifBlank { "image/*" }
                putExtra(Intent.EXTRA_STREAM, uris.first())
                clipData = ClipData.newRawUri(items.first().first.filename, uris.first())
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
        } else {
            val clip = ClipData.newRawUri(items.first().first.filename, uris.first()).apply {
                for (i in 1 until uris.size) {
                    addItem(ClipData.Item(uris[i]))
                }
            }
            Intent(Intent.ACTION_SEND_MULTIPLE).apply {
                type = "image/*"
                putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris)
                clipData = clip
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
        }

        val chooser = Intent.createChooser(intent, "Share ${items.size} image(s)").apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        context.startActivity(chooser)
    }

    suspend fun saveFileToDownloads(
        context: Context,
        file: DriveTransferFile,
        localFile: File,
    ): Boolean = withContext(Dispatchers.IO) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val resolver = context.contentResolver
            val values = ContentValues().apply {
                put(MediaStore.Downloads.DISPLAY_NAME, file.filename)
                put(MediaStore.Downloads.MIME_TYPE, file.mimeType.ifBlank { "application/octet-stream" })
                put(MediaStore.Downloads.RELATIVE_PATH, "${Environment.DIRECTORY_DOWNLOADS}/koma")
                put(MediaStore.Downloads.IS_PENDING, 1)
            }
            val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
                ?: return@withContext false
            try {
                resolver.openOutputStream(uri)?.use { out ->
                    localFile.inputStream().use { input -> input.copyTo(out) }
                }
                val done = ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }
                resolver.update(uri, done, null, null)
                true
            } catch (_: Throwable) {
                resolver.delete(uri, null, null)
                false
            }
        } else {
            @Suppress("DEPRECATION")
            val dir = File(
                Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS),
                "koma"
            ).apply { mkdirs() }
            localFile.copyTo(File(dir, file.filename), overwrite = true)
            true
        }
    }

    fun shareTransferFile(context: Context, file: DriveTransferFile, localFile: File) {
        val authority = "${context.packageName}.fileprovider"
        val uri = FileProvider.getUriForFile(context, authority, localFile)
        val intent = Intent(Intent.ACTION_SEND).apply {
            type = file.mimeType.ifBlank { "*/*" }
            putExtra(Intent.EXTRA_STREAM, uri)
            clipData = ClipData.newRawUri(file.filename, uri)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        val chooser = Intent.createChooser(intent, "Share ${file.filename}").apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        context.startActivity(chooser)
    }
}
