package app.opencleaner

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Favorite
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.util.concurrent.TimeUnit

private fun Kind.title() = when (this) {
    Kind.Duplicate -> R.string.kind_duplicate; Kind.Screenshot -> R.string.kind_screenshot
    Kind.Blurry -> R.string.kind_blurry; Kind.TextImage -> R.string.kind_text
    Kind.ChatMedia -> R.string.kind_chat; Kind.LargeVideo -> R.string.kind_large_video
    Kind.UnusedApp -> R.string.kind_unused_app
}

@Composable
fun HomeScreen(
    vm: MainVm, mediaGranted: Boolean, busy: Boolean, size: (Long) -> String,
    onGrantMedia: () -> Unit, onUsageSettings: () -> Unit, onRescan: () -> Unit, onClean: () -> Unit,
) = OpenCleanerTheme {
    var open by remember { mutableStateOf<Kind?>(null) }
    BackHandler(open != null) { open = null }
    val picked = remember(vm.items) { vm.picked }
    val groups = remember(vm.items) { vm.items.groupBy { it.kind } }

    Box(Modifier.fillMaxSize().backdrop(Look.dashboard)) {
        Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.systemBars).padding(horizontal = 16.dp)) {
            val kind = open
            if (kind == null) {
                LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(12.dp), contentPadding = PaddingValues(top = 16.dp, bottom = 16.dp)) {
                    item { Hero(picked, vm.progress, size) }
                    if (!mediaGranted) item { Banner(R.string.need_media, R.string.grant, onGrantMedia) }
                    if (!vm.usageAccess) item { Banner(R.string.need_usage, R.string.open_settings, onUsageSettings) }
                    if (!busy && vm.items.isEmpty() && (mediaGranted || vm.usageAccess)) item { Text(stringResource(R.string.nothing), color = Look.dim, modifier = Modifier.padding(24.dp)) }
                    // biggest savings first
                    for (k in Kind.entries.sortedByDescending { k -> groups[k]?.sumOf { it.bytes } ?: 0L }) groups[k]?.let { list -> item { GroupCard(k, list, size) { open = k } } }
                    if (vm.protectedCount > 0) item { Protected(vm.protectedCount) }
                }
            } else {
                Review(kind, groups[kind].orEmpty(), size, Modifier.weight(1f), vm, back = { open = null })
            }
            BottomBar(picked, busy, size, onRescan, onClean)
        }
    }
}

/** What a tap on Clean would free, plus the progress of the on-device analysis. */
@Composable private fun Hero(picked: List<Item>, progress: Pair<Int, Int>?, size: (Long) -> String) =
    Column(Modifier.fillMaxWidth().padding(vertical = 8.dp)) {
        Text(stringResource(R.string.app_name), color = Look.dim, fontSize = 14.sp, fontWeight = FontWeight.Medium)
        Text(size(picked.sumOf { it.bytes }), color = Color.White, fontSize = 48.sp, fontWeight = FontWeight.Bold)
        Text(if (picked.isEmpty()) stringResource(R.string.hero_none) else stringResource(R.string.hero_caption, picked.size), color = Look.dim, fontSize = 14.sp)
        progress?.let { (done, total) ->
            LinearProgressIndicator({ done / total.toFloat() }, Modifier.fillMaxWidth().padding(top = 14.dp).clip(CircleShape), color = Look.ok, trackColor = Color.White.copy(alpha = 0.2f))
            Text(stringResource(R.string.analyzing, done, total), color = Look.dim, fontSize = 12.sp, modifier = Modifier.padding(top = 6.dp))
        }
    }

@Composable private fun Protected(n: Int) =
    Row(Modifier.fillMaxWidth().glass(20, 0.08f).padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
        Icon(Icons.Outlined.Favorite, null, tint = Look.ok, modifier = Modifier.size(22.dp))
        Text(stringResource(R.string.protected_note, n), color = Look.dim, fontSize = 13.sp, modifier = Modifier.padding(start = 12.dp))
    }

/** The big round button, dead center and in the same place on every page. */
@Composable private fun BottomBar(picked: List<Item>, busy: Boolean, size: (Long) -> String, onScan: () -> Unit, onClean: () -> Unit) =
    Column(Modifier.fillMaxWidth().padding(vertical = 12.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        if (picked.isNotEmpty()) Text(stringResource(R.string.clean_button, picked.size, size(picked.sumOf { it.bytes })), color = Look.dim, fontSize = 13.sp, modifier = Modifier.padding(bottom = 8.dp))
        BigButton(stringResource(if (picked.isEmpty()) R.string.scan else R.string.clean), busy, if (picked.isEmpty()) onScan else onClean)
        if (picked.isNotEmpty()) Text(stringResource(R.string.clean_note), color = Look.dim, fontSize = 12.sp, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 8.dp))
    }

@Composable private fun Banner(text: Int, action: Int, onClick: () -> Unit) =
    Row(Modifier.fillMaxWidth().glass().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(stringResource(text), color = Color.White, modifier = Modifier.weight(1f).padding(end = 12.dp)); Pill(stringResource(action), onClick)
    }

/** One group on the home page: title, totals and a small preview strip. Tap to review. */
@Composable private fun GroupCard(kind: Kind, list: List<Item>, size: (Long) -> String, onOpen: () -> Unit) {
    val ticked = list.count { it.selected }
    val (c1, c2) = Look.palette(kind)
    val shape = RoundedCornerShape(26.dp)
    Column(Modifier.fillMaxWidth().clip(shape).background(Brush.linearGradient(listOf(c1.copy(alpha = 0.55f), c2.copy(alpha = 0.40f)))).border(0.5.dp, Color.White.copy(alpha = 0.22f), shape)
        .clickable(onClick = onOpen).padding(16.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            GroupIcon(kind)
            Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                Text(stringResource(kind.title()), color = Color.White, fontWeight = FontWeight.SemiBold)
                Text("${list.size} · ${size(list.sumOf { it.bytes })}" + if (ticked > 0) " · " + stringResource(R.string.selected_count, ticked) else "", color = Look.dim, fontSize = 12.sp)
            }
            Pill(stringResource(R.string.review), onOpen)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            list.take(5).forEach { Thumb(it, Modifier.weight(1f).aspectRatio(1f).clip(RoundedCornerShape(12.dp)), px = 128) }
            repeat(5 - list.size.coerceAtMost(5)) { Spacer(Modifier.weight(1f)) }
        }
    }
}

/** All items of one group, to tick or untick. Photos as a grid, apps as rows. */
@Composable private fun Review(kind: Kind, list: List<Item>, size: (Long) -> String, modifier: Modifier, vm: MainVm, back: () -> Unit) {
    val all = remember(list) { list.filter { !it.keep }.all { it.selected } }
    Row(Modifier.fillMaxWidth().padding(top = 16.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Pill(stringResource(R.string.back), back)
        Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
            Text(stringResource(kind.title()), color = Color.White, fontWeight = FontWeight.SemiBold)
            Text("${list.size} · ${size(list.sumOf { it.bytes })}", color = Look.dim, fontSize = 12.sp)
        }
        Pill(stringResource(if (all) R.string.select_none else R.string.select_all)) { vm.setKind(kind, !all) }
    }
    LazyVerticalGrid(GridCells.Fixed(3), modifier, horizontalArrangement = Arrangement.spacedBy(4.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        if (kind == Kind.UnusedApp) items(list, key = { it.key }, span = { GridItemSpan(3) }) { AppRow(it, size) { vm.toggle(it.key) } }
        else items(list, key = { it.key }) { Cell(it) { vm.toggle(it.key) } }
    }
}

@Composable private fun Cell(item: Item, onClick: () -> Unit) {
    val shape = RoundedCornerShape(10.dp)
    val ring = if (item.selected) Look.ok else Color.White.copy(alpha = 0.25f)
    Box(Modifier.aspectRatio(1f).clip(shape).border(if (item.selected) 2.dp else 0.5.dp, ring, shape).clickable(onClick = onClick)) {
        Thumb(item, Modifier.fillMaxSize().alpha(if (item.selected) 1f else 0.6f))
        if (item.keep) Icon(Icons.Outlined.Favorite, null, tint = Look.ok, modifier = Modifier.align(Alignment.BottomStart).padding(6.dp).size(16.dp))
        Checkbox(item.selected, { onClick() }, Modifier.align(Alignment.TopEnd), colors = Look.checks())
    }
}

@Composable private fun AppRow(item: Item, size: (Long) -> String, onClick: () -> Unit) {
    val days = if (item.lastUsed > 0) TimeUnit.MILLISECONDS.toDays(System.currentTimeMillis() - item.lastUsed).toInt() else null
    Row(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Thumb(item, Modifier.size(40.dp), px = 96)
        Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
            Text(item.label, color = Color.White)
            Text(if (days != null) stringResource(R.string.unused_days, days) else stringResource(R.string.never_used), color = Look.dim, fontSize = 12.sp)
        }
        Text(size(item.bytes), color = Look.dim, fontSize = 12.sp)
        Checkbox(item.selected, { onClick() }, colors = Look.checks())
    }
}
