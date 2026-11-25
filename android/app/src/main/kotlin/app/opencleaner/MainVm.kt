package app.opencleaner

import android.app.Application
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.*
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit

class MainVm(private val app: Application) : AndroidViewModel(app) {
    var items by mutableStateOf<List<Item>>(emptyList()); private set
    var scanning by mutableStateOf(false); private set
    /** (done, total) while photos are being analyzed on the phone, else null. */
    var progress by mutableStateOf<Pair<Int, Int>?>(null); private set
    var usageAccess by mutableStateOf(Scanner.hasUsageAccess(app)); private set
    /** Photos of people and pets found so far, listed or not. */
    var protectedCount by mutableStateOf(0); private set

    val picked get() = items.filter { it.selected }

    private var media = listOf<Media>()
    private var apps = listOf<Item>()
    private val infos = HashMap<Long, Info>()
    private val overrides = HashMap<String, Boolean>() // what the user ticked by hand wins over the defaults
    private val removed = HashSet<String>()
    private var job: Job? = null

    fun scan(mediaGranted: Boolean) {
        job?.cancel()
        job = viewModelScope.launch(Dispatchers.Default) {
            scanning = true
            usageAccess = Scanner.hasUsageAccess(app)
            media = if (mediaGranted) Scanner.media(app.contentResolver) else emptyList()
            apps = if (usageAccess) Scanner.apps(app) else emptyList()
            infos.clear()
            val cache = InfoCache.load(app)
            media.forEach { m -> cache[m.id]?.takeIf { it.modified == m.modified }?.let { infos[m.id] = it } }
            publish(); scanning = false
            analyze()
        }
    }

    private suspend fun analyze() {
        val todo = media.filter { !it.video && it.id !in infos }
        if (todo.isEmpty()) return
        val analyzer = Analyzer(app)
        try {
            val gate = Semaphore(3)
            var done = 0
            progress = 0 to todo.size
            for (batch in todo.chunked(60)) {
                coroutineScope {
                    batch.map { m -> async { gate.withPermit { analyzer.analyze(m) }?.let { synchronized(infos) { infos[m.id] = it } } } }.awaitAll()
                }
                done += batch.size
                progress = done to todo.size
                publish()
            }
            InfoCache.save(app, infos)
        } finally { analyzer.close(); progress = null }
    }

    private fun publish() {
        protectedCount = synchronized(infos) { infos.values.count { it.keep } }
        items = (synchronized(infos) { Smart.classify(media, infos) } + apps)
            .filter { it.key !in removed }
            .map { i -> overrides[i.key]?.let { i.copy(selected = it) } ?: i }
    }

    fun toggle(key: String) {
        val i = items.first { it.key == key }
        overrides[key] = !i.selected
        items = items.map { if (it.key == key) it.copy(selected = !it.selected) else it }
    }

    /** "Select all" leaves out photos of people and pets; "none" clears everything in the group. */
    fun setKind(kind: Kind, on: Boolean) {
        items = items.map {
            if (it.kind != kind || (on && it.keep)) it else it.copy(selected = on).also { n -> overrides[n.key] = on }
        }
    }

    fun drop(keys: Set<String>) { removed += keys; publish() }
}
