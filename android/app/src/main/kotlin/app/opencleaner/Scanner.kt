package app.opencleaner

import android.app.AppOpsManager
import android.app.usage.StorageStatsManager
import android.app.usage.UsageStatsManager
import android.content.ContentResolver
import android.content.ContentUris
import android.content.Context
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.os.Process
import android.os.storage.StorageManager
import android.provider.MediaStore
import java.util.concurrent.TimeUnit

/** Finds candidates. Never deletes: removal goes through system dialogs (see Cleaner in MainActivity). */
object Scanner {
    private val day = TimeUnit.DAYS.toMillis(1)
    private const val LARGE_VIDEO = 200L shl 20

    fun media(cr: ContentResolver): List<Media> {
        val out = ArrayList<Media>()
        for (video in listOf(false, true)) {
            val base = if (video) MediaStore.Video.Media.EXTERNAL_CONTENT_URI else MediaStore.Images.Media.EXTERNAL_CONTENT_URI
            val cols = arrayOf("_id", "_display_name", "_size", "relative_path", "date_added", "date_modified", "width", "height") // MediaStore.MediaColumns
            cr.query(base, cols, null, null, null)?.use { q ->
                while (q.moveToNext()) {
                    val m = Media(q.getLong(0), ContentUris.withAppendedId(base, q.getLong(0)), q.getString(1) ?: "", q.getLong(2),
                        q.getString(3) ?: "", q.getLong(4), q.getLong(5), video, q.getInt(6), q.getInt(7))
                    if (video && m.size < LARGE_VIDEO && !m.chat) continue // other videos are not junk
                    out += m
                }
            }
        }
        return out.sortedByDescending { it.added }
    }

    fun hasUsageAccess(ctx: Context): Boolean {
        val ops = ctx.getSystemService(AppOpsManager::class.java)
        return ops.unsafeCheckOpNoThrow(AppOpsManager.OPSTR_GET_USAGE_STATS, Process.myUid(), ctx.packageName) == AppOpsManager.MODE_ALLOWED
    }

    fun apps(ctx: Context): List<Item> {
        val pm = ctx.packageManager
        val now = System.currentTimeMillis()
        val usage = ctx.getSystemService(UsageStatsManager::class.java).queryAndAggregateUsageStats(now - 365 * day, now)
        val ss = ctx.getSystemService(StorageStatsManager::class.java)
        val launcher = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
        return pm.queryIntentActivities(launcher, 0)
            .map { it.activityInfo.applicationInfo }
            .distinctBy { it.packageName }
            .filter { it.flags and ApplicationInfo.FLAG_SYSTEM == 0 && it.packageName != ctx.packageName }
            .mapNotNull { ai ->
                val last = usage[ai.packageName]?.lastTimeUsed ?: 0L
                if (last > now - 90 * day) return@mapNotNull null // used recently
                val bytes = runCatching {
                    ss.queryStatsForPackage(StorageManager.UUID_DEFAULT, ai.packageName, Process.myUserHandle())
                        .let { it.appBytes + it.dataBytes + it.cacheBytes }
                }.getOrDefault(0L)
                Item("a${ai.packageName}", Kind.UnusedApp, ai.loadLabel(pm).toString(), bytes, false, pkg = ai.packageName, lastUsed = last)
            }
            .sortedByDescending { it.bytes }
    }
}
