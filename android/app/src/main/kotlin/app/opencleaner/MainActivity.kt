package app.opencleaner

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.provider.MediaStore
import android.provider.Settings
import android.text.format.Formatter
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext

class MainActivity : ComponentActivity() {
    private val vm: MainVm by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge(statusBarStyle = SystemBarStyle.dark(android.graphics.Color.TRANSPARENT), navigationBarStyle = SystemBarStyle.dark(android.graphics.Color.TRANSPARENT))
        setContent { Root(vm) }
    }
}

// READ_MEDIA_* exists from API 33; Android 11-12 (minSdk 30) use READ_EXTERNAL_STORAGE
private val mediaPerms =
    if (android.os.Build.VERSION.SDK_INT >= 33) arrayOf(Manifest.permission.READ_MEDIA_IMAGES, Manifest.permission.READ_MEDIA_VIDEO)
    else arrayOf(Manifest.permission.READ_EXTERNAL_STORAGE)

private fun mediaGranted(a: android.content.Context) =
    mediaPerms.all { a.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED }

@Composable
private fun Root(vm: MainVm) {
    val ctx = LocalContext.current
    var granted by remember { mutableStateOf(mediaGranted(ctx)) }
    // Uninstall queue: one system dialog per app, chained so the user only taps OK.
    var queue by remember { mutableStateOf(listOf<Item>()) }

    val uninstall = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { r ->
        val done = queue.firstOrNull()
        if (done != null && r.resultCode == Activity.RESULT_OK) vm.drop(setOf(done.key))
        queue = queue.drop(1)
    }
    LaunchedEffect(queue) {
        queue.firstOrNull()?.let {
            uninstall.launch(Intent(Intent.ACTION_DELETE, Uri.parse("package:${it.pkg}")).putExtra(Intent.EXTRA_RETURN_RESULT, true))
        }
    }

    val trash = rememberLauncherForActivityResult(ActivityResultContracts.StartIntentSenderForResult()) { r ->
        if (r.resultCode == Activity.RESULT_OK) vm.drop(vm.picked.filter { it.uri != null }.map { it.key }.toSet())
        queue = vm.picked.filter { it.pkg != null }
    }
    val askMedia = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        granted = mediaGranted(ctx); vm.scan(granted)
    }

    LaunchedEffect(Unit) { vm.scan(granted) }

    val onClean = {
        val uris = vm.picked.mapNotNull { it.uri }
        if (uris.isNotEmpty()) {
            // One confirmation for the whole batch; items stay recoverable for 30 days.
            val pi = MediaStore.createTrashRequest(ctx.contentResolver, uris, true)
            trash.launch(IntentSenderRequest.Builder(pi.intentSender).build())
        } else queue = vm.picked.filter { it.pkg != null }
    }

    HomeScreen(
        vm = vm,
        mediaGranted = granted,
        size = { Formatter.formatShortFileSize(ctx, it) },
        onGrantMedia = { askMedia.launch(mediaPerms) },
        onUsageSettings = { ctx.startActivity(Intent(Settings.ACTION_USAGE_ACCESS_SETTINGS)) },
        onRescan = { vm.scan(granted) },
        busy = vm.scanning || queue.isNotEmpty(),
        onClean = onClean,
    )
}
