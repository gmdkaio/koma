package dev.koma.app.data

import org.junit.Assert.assertEquals
import org.junit.Test

class ModelsTest {
    @Test
    fun slugifyNormalizesChannelNamesLikePcApp() {
        assertEquals("ynu", slugifyChannel("  YnU "))
        assertEquals("blue_box", slugifyChannel("Blue--Box!"))
        assertEquals("re_zero_2", slugifyChannel("Re:Zero 2"))
    }

    @Test
    fun nextChannelFilenameAdvancesPastHighestSequence() {
        val existing = listOf("rei_0001.png", "rei_0042.jpg", "custom_shot.png")
        assertEquals("rei_0043.png", nextChannelFilename("rei", existing, "png"))
        assertEquals("asuka_0001.jpg", nextChannelFilename("asuka", emptyList(), ".JPG"))
    }

    @Test
    fun extFromMimeFallsBackToFilename() {
        assertEquals("jpg", extFromMime("image/jpeg", "foo.bin"))
        assertEquals("webp", extFromMime(null, "panel.WEBP"))
        assertEquals("png", extFromMime("application/octet-stream", null))
    }

    @Test
    fun sizedThumbnailUrlUpgradesDriveSizeParam() {
        val img = DriveImage(
            id = "1",
            channelId = "c1",
            channelSlug = "rei",
            filename = "rei_0001.png",
            mimeType = "image/png",
            thumbnailLink = "https://lh3.googleusercontent.com/drive-storage/abc=s220",
        )
        assertEquals(
            "https://lh3.googleusercontent.com/drive-storage/abc=s480",
            img.sizedThumbnailUrl(480),
        )
    }
}
