package app.opencleaner

import java.util.concurrent.TimeUnit

/** Turns media plus what the analysis found into cleaning candidates. Pure: no I/O, no deletes. */
object Smart {
    const val BLURRY = 30f          // Laplacian variance below this: blurry or too dark
    const val TEXT_BLOCKS = 6       // this many text blocks: a meme, receipt or document
    private const val LARGE_VIDEO = 200L shl 20
    private val old = TimeUnit.DAYS.toSeconds(30)

    fun classify(ms: List<Media>, info: Map<Long, Info>): List<Item> {
        val extra = extraCopies(ms, info)
        val cutoff = System.currentTimeMillis() / 1000 - old
        return ms.mapNotNull { m ->
            val i = info[m.id]
            val (kind, on) = when {
                m.video -> if (m.size >= LARGE_VIDEO) Kind.LargeVideo to false else Kind.ChatMedia to false
                m.id in extra -> Kind.Duplicate to true
                m.shot -> Kind.Screenshot to (m.added < cutoff)
                i != null && i.blur < BLURRY -> Kind.Blurry to true
                i != null && i.text >= TEXT_BLOCKS -> Kind.TextImage to m.chat // a receipt in Camera is left alone
                m.chat -> Kind.ChatMedia to false
                else -> return@mapNotNull null
            }
            // People and pets are listed but never ticked. Screenshots are the exception: an avatar in a chat is not family.
            val keep = i?.keep == true && !m.shot
            Item(m.key, kind, m.name, m.size, on && !keep, uri = m.uri, keep = keep)
        }.sortedByDescending { it.bytes }
    }

    /** Ids of photos that are extra copies of a better one (same picture, near-identical hash). */
    private fun extraCopies(ms: List<Media>, info: Map<Long, Info>): Set<Long> {
        val imgs = ms.filter { !it.video && !it.shot && info[it.id] != null }
        val hash = LongArray(imgs.size) { info.getValue(imgs[it].id).hash }
        val parent = IntArray(imgs.size) { it }
        fun find(x: Int): Int { var r = x; while (parent[r] != r) r = parent[r]; return r }
        // Pigeonhole: two hashes within 3 bits agree exactly on at least one of 5 chunks, so only compare inside a chunk bucket.
        for (c in 0 until 5) {
            val shift = c * 13; val mask = if (c < 4) 0x1FFFL else 0xFFFL
            val buckets = HashMap<Long, MutableList<Int>>()
            for (k in imgs.indices) buckets.getOrPut((hash[k] ushr shift) and mask) { mutableListOf() } += k
            // ponytail: buckets over 300 (blank images) are skipped; raise the cap if real duplicates hide there
            for (b in buckets.values) if (b.size in 2..300) for (x in b.indices) for (y in x + 1 until b.size) {
                val p = b[x]; val q = b[y]
                if (java.lang.Long.bitCount(hash[p] xor hash[q]) <= 3 && imgs[p].w * imgs[q].h == imgs[q].w * imgs[p].h) parent[find(p)] = find(q)
            }
        }
        val out = HashSet<Long>()
        imgs.indices.groupBy { find(it) }.values.filter { it.size > 1 }.forEach { set ->
            // keep the biggest picture, then the largest file, then the oldest
            val best = set.maxWith(compareBy<Int>({ imgs[it].w * imgs[it].h }, { imgs[it].size }, { -imgs[it].modified }))
            set.filter { it != best }.forEach { out += imgs[it].id }
        }
        return out
    }
}
