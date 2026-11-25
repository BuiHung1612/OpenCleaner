package app.opencleaner

import android.util.Size
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.graphics.drawable.toBitmap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** Big round call-to-action (macOS `BigButton`): 104dp glass circle with a white ring. */
@Composable fun BigButton(title: String, busy: Boolean, onClick: () -> Unit) =
    Box(Modifier.size(104.dp).border(2.dp, Color.White.copy(alpha = 0.45f), CircleShape).padding(6.dp)
        .clip(CircleShape).glassCircle().clickable(enabled = !busy, onClick = onClick), Alignment.Center) {
        if (busy) CircularProgressIndicator(color = Color.White, modifier = Modifier.size(36.dp))
        else Text(title, color = Color.White, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
    }

/** Secondary action (macOS `.pill`): glass capsule, white label. */
@Composable fun Pill(text: String, onClick: () -> Unit) =
    Box(Modifier.heightIn(min = 40.dp).glass(20, 0.16f).clickable(onClick = onClick).padding(horizontal = 20.dp), Alignment.Center) {
        Text(text, color = Color.White, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
    }

val Kind.icon get() = when (this) {
    Kind.Duplicate -> Icons.Outlined.ContentCopy; Kind.Screenshot -> Icons.Outlined.Screenshot
    Kind.Blurry -> Icons.Outlined.BlurOn; Kind.TextImage -> Icons.Outlined.TextSnippet
    Kind.ChatMedia -> Icons.Outlined.Forum; Kind.LargeVideo -> Icons.Outlined.Videocam
    Kind.UnusedApp -> Icons.Outlined.Apps
}

/** Rounded square in the group's gradient with its icon, like the macOS sidebar icons. */
@Composable fun GroupIcon(kind: Kind) {
    val shape = androidx.compose.foundation.shape.RoundedCornerShape(14.dp)
    Box(Modifier.size(44.dp).clip(shape).background(Brush.linearGradient(Look.palette(kind).toList())).border(0.5.dp, Color.White.copy(alpha = 0.4f), shape), Alignment.Center) {
        Icon(kind.icon, null, tint = Color.White, modifier = Modifier.size(24.dp))
    }
}

/** Decoded thumbnails, 32 MB: scrolling back must not decode again. */
private val thumbs = object : android.util.LruCache<String, ImageBitmap>(32 shl 20) {
    override fun sizeOf(key: String, v: ImageBitmap) = v.asAndroidBitmap().byteCount
}

/** Thumbnail of a media item, loaded off the main thread. */
@Composable fun Thumb(item: Item, modifier: Modifier = Modifier, px: Int = 256) {
    val ctx = LocalContext.current
    val id = "${item.key}@$px"
    val bmp by produceState<ImageBitmap?>(thumbs[id], id) {
        if (value != null) return@produceState
        value = withContext(Dispatchers.IO) {
            runCatching {
                if (item.uri != null) ctx.contentResolver.loadThumbnail(item.uri, Size(px, px), null).asImageBitmap()
                else ctx.packageManager.getApplicationIcon(item.pkg!!).toBitmap(px, px).asImageBitmap()
            }.getOrNull()?.also { thumbs.put(id, it) }
        }
    }
    bmp?.let { Image(it, null, modifier, contentScale = ContentScale.Crop) } ?: Box(modifier.glass(8, 0.08f))
}
