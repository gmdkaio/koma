package dev.koma.app.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import dev.koma.app.data.KomaTheme

@Immutable
data class KomaPalette(
    val bg: Color,
    val surface: Color,
    val surfaceElevated: Color,
    val border: Color,
    val fg: Color,
    val muted: Color,
    val accent: Color,
    val accentFg: Color,
    val selectionBezel: Color,
    val selectionRim: Color,
    val danger: Color,
    val dangerBg: Color,
    val syncOk: Color,
    val syncBusy: Color,
    val isDark: Boolean,
)

val ObsidianPalette = KomaPalette(
    bg = Color(0xFF0A0B0F),
    surface = Color(0xFF111317),
    surfaceElevated = Color(0xFF171A21),
    border = Color(0xFF232833),
    fg = Color(0xFFE2E5EC),
    muted = Color(0xFF8B93A3),
    accent = Color(0xFFD8DCE4),
    accentFg = Color(0xFF08090C),
    selectionBezel = Color(0xFF161920),
    selectionRim = Color(0x55D2DAE8),
    danger = Color(0xFFF87171),
    dangerBg = Color(0x33EF4444),
    syncOk = Color(0xFF4ADE80),
    syncBusy = Color(0xFFFBBF24),
    isDark = true,
)

val MangaPaperPalette = KomaPalette(
    bg = Color(0xFFE8DDCA),
    surface = Color(0xFFDCCEB8),
    surfaceElevated = Color(0xFFD6C7AF),
    border = Color(0xFFC2AE90),
    fg = Color(0xFF261B14),
    muted = Color(0xFF5D4738),
    accent = Color(0xFF3D281C),
    accentFg = Color(0xFFF5ECE0),
    selectionBezel = Color(0xFF3D281C),
    selectionRim = Color(0xFFE8DDCA),
    danger = Color(0xFFB82D2D),
    dangerBg = Color(0x26B82D2D),
    syncOk = Color(0xFF1F7A4C),
    syncBusy = Color(0xFFB4690E),
    isDark = false,
)

val LocalKomaPalette = staticCompositionLocalOf { ObsidianPalette }

@Composable
fun KomaAppTheme(
    theme: KomaTheme,
    content: @Composable () -> Unit,
) {
    val palette = when (theme) {
        KomaTheme.OBSIDIAN -> ObsidianPalette
        KomaTheme.MANGA_PAPER -> MangaPaperPalette
    }

    val colorScheme = if (palette.isDark) {
        darkColorScheme(
            primary = palette.accent,
            onPrimary = palette.accentFg,
            background = palette.bg,
            onBackground = palette.fg,
            surface = palette.surface,
            onSurface = palette.fg,
            surfaceVariant = palette.surfaceElevated,
            onSurfaceVariant = palette.muted,
            outline = palette.border,
            error = palette.danger,
        )
    } else {
        lightColorScheme(
            primary = palette.accent,
            onPrimary = palette.accentFg,
            background = palette.bg,
            onBackground = palette.fg,
            surface = palette.surface,
            onSurface = palette.fg,
            surfaceVariant = palette.surfaceElevated,
            onSurfaceVariant = palette.muted,
            outline = palette.border,
            error = palette.danger,
        )
    }

    CompositionLocalProvider(LocalKomaPalette provides palette) {
        MaterialTheme(
            colorScheme = colorScheme,
            content = content,
        )
    }
}
