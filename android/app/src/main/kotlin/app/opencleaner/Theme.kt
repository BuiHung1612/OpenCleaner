package app.opencleaner

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CheckboxColors
import androidx.compose.material3.CheckboxDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp

/** Same look as the macOS and Windows apps: two-color gradient, glass panels, white text, mint accents. */
object Look {
    val dim = Color.White.copy(alpha = 0.88f)
    val ok = Color(0xFF7CF2C8)
    val warn = Color(0xFFFFD54F)
    val ink = Color(0xFF14202B)

    /** Colors from shared/tools.json, as on the macOS pages. */
    val dashboard = Color(0xFF3A3FB8) to Color(0xFF1B1446)
    fun palette(kind: Kind) = when (kind) {
        Kind.Screenshot -> Color(0xFF0A6A66) to Color(0xFF14366B)   // clean
        Kind.Duplicate -> Color(0xFF5B4B8A) to Color(0xFF1E1A3C)    // home_items
        Kind.Blurry -> Color(0xFF2F5D8A) to Color(0xFF1A2140)       // devtools
        Kind.TextImage -> Color(0xFF9D4C28) to Color(0xFF5B1F4A)    // installer
        Kind.ChatMedia -> Color(0xFF28794C) to Color(0xFF1A2F5A)    // duplicates
        Kind.LargeVideo -> Color(0xFF936B09) to Color(0xFF5A2A12)   // large_old
        Kind.UnusedApp -> Color(0xFF2961BA) to Color(0xFF2A1857)    // uninstaller
    }

    @Composable fun checks(): CheckboxColors = CheckboxDefaults.colors(
        checkedColor = ok, checkmarkColor = ink, uncheckedColor = Color.White.copy(alpha = 0.7f),
    )
}

@Composable fun OpenCleanerTheme(content: @Composable () -> Unit) = MaterialTheme(
    colorScheme = darkColorScheme(primary = Look.ok, onSurface = Color.White, onBackground = Color.White, surface = Color(0xFF1B1446)),
    content = content,
)

/** Full-screen gradient with two soft light blobs, like macOS `Backdrop`. */
fun Modifier.backdrop(colors: Pair<Color, Color>): Modifier = this.drawBehind {
    drawRect(Brush.linearGradient(listOf(colors.first, colors.second), Offset.Zero, Offset(size.width, size.height)))
    drawRect(Brush.radialGradient(listOf(colors.first.copy(alpha = 0.9f), Color.Transparent), Offset(size.width * 0.85f, size.height * 0.1f), size.maxDimension * 0.6f))
    drawRect(Brush.radialGradient(listOf(Color.White.copy(alpha = 0.10f), Color.Transparent), Offset(size.width * 0.2f, size.height * 0.95f), size.maxDimension * 0.5f))
}

/** Frosted-glass panel (no real blur: a light tint and a hairline edge). */
fun Modifier.glass(radius: Int = 22, tint: Float = 0.12f): Modifier {
    val s = RoundedCornerShape(radius.dp)
    return this.background(Color.White.copy(alpha = tint), s).border(0.5.dp, Color.White.copy(alpha = 0.25f), s)
}

fun Modifier.glassCircle(): Modifier =
    this.background(Color.White.copy(alpha = 0.18f), CircleShape).border(0.5.dp, Color.White.copy(alpha = 0.25f), CircleShape)
