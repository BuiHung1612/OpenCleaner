package app.opencleaner

import android.net.Uri

enum class Kind { Duplicate, Screenshot, Blurry, TextImage, ChatMedia, LargeVideo, UnusedApp }

private val chatDirs = listOf("Zalo", "Telegram", "WhatsApp", "Messenger", "Viber")

/** A photo or video as MediaStore lists it. */
data class Media(
    val id: Long, val uri: Uri, val name: String, val size: Long, val path: String,
    val added: Long, val modified: Long, val video: Boolean, val w: Int, val h: Int,
) {
    val key get() = "m$id${if (video) 'v' else 'i'}"
    val chat get() = chatDirs.any { path.contains(it, true) }
    val shot get() = !video && path.contains("Screenshots", true)
}

/** What the on-device analysis found out about one photo. Cached on disk by (id, modified). */
data class Info(val modified: Long, val faces: Int, val keep: Boolean, val blur: Float, val hash: Long, val text: Int)

/** One thing the user may clean: a media file (uri) or an app (pkg). */
data class Item(
    val key: String,
    val kind: Kind,
    val label: String,
    val bytes: Long,
    val selected: Boolean,
    val uri: Uri? = null,
    val pkg: String? = null,
    val lastUsed: Long = 0, // apps only; 0 = unknown
    val keep: Boolean = false, // shows people or pets: never ticked for the user
)
