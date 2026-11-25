package app.opencleaner

import android.content.Context
import android.graphics.Bitmap
import android.util.Size
import com.google.android.gms.tasks.Tasks
import com.google.mlkit.vision.common.InputImage
import com.google.mlkit.vision.face.FaceDetection
import com.google.mlkit.vision.face.FaceDetectorOptions
import com.google.mlkit.vision.label.ImageLabeling
import com.google.mlkit.vision.label.defaults.ImageLabelerOptions
import com.google.mlkit.vision.text.TextRecognition
import com.google.mlkit.vision.text.latin.TextRecognizerOptions
import java.io.File

/** Reads one photo on the phone: faces, labels, text, sharpness and a perceptual hash. Call off the main thread. */
class Analyzer(private val ctx: Context) {
    private val faces = FaceDetection.getClient(FaceDetectorOptions.Builder().setPerformanceMode(FaceDetectorOptions.PERFORMANCE_MODE_FAST).setMinFaceSize(0.1f).build())
    private val labeler = ImageLabeling.getClient(ImageLabelerOptions.Builder().setConfidenceThreshold(0.65f).build())
    private val ocr = TextRecognition.getClient(TextRecognizerOptions.DEFAULT_OPTIONS)

    fun analyze(m: Media): Info? = runCatching {
        var bmp = ctx.contentResolver.loadThumbnail(m.uri, Size(320, 320), null)
        if (bmp.config == Bitmap.Config.HARDWARE) bmp = bmp.copy(Bitmap.Config.ARGB_8888, false)
        val img = InputImage.fromBitmap(bmp, 0)
        val nFaces = Tasks.await(faces.process(img)).size
        val keep = nFaces > 0 || Tasks.await(labeler.process(img)).any { it.text in KEEP_LABELS }
        val text = if (keep) 0 else Tasks.await(ocr.process(img)).textBlocks.size
        Info(m.modified, nFaces, keep, sharpness(bmp), dhash(bmp), text)
    }.getOrNull()

    fun close() { faces.close(); labeler.close(); ocr.close() }

    companion object {
        // Labels of ML Kit's base model that mean "someone will want this photo".
        private val KEEP_LABELS = setOf("Person", "Baby", "Toddler", "Child", "Selfie", "Smile", "Face", "Facial expression", "Family",
            "Dog", "Cat", "Pet", "Wedding", "Bride", "Birthday", "Party", "Bridal", "Girl", "Boy", "Hug", "Kiss")

        private fun gray(p: Int) = ((p shr 16 and 255) * 3 + (p shr 8 and 255) * 6 + (p and 255)) / 10

        /** Variance of the Laplacian on a 128px copy: low = blurry or nearly black. */
        fun sharpness(b: Bitmap): Float {
            val w = 128; val h = (b.height * w / b.width).coerceAtLeast(3)
            val px = IntArray(w * h).also { Bitmap.createScaledBitmap(b, w, h, true).getPixels(it, 0, w, 0, 0, w, h) }
            val g = IntArray(w * h) { gray(px[it]) }
            var sum = 0.0; var sq = 0.0; var n = 0
            for (y in 1 until h - 1) for (x in 1 until w - 1) {
                val i = y * w + x
                val l = (4 * g[i] - g[i - 1] - g[i + 1] - g[i - w] - g[i + w]).toDouble()
                sum += l; sq += l * l; n++
            }
            val mean = sum / n
            return (sq / n - mean * mean).toFloat()
        }

        /** 64-bit difference hash: near-identical photos differ in few bits. */
        fun dhash(b: Bitmap): Long {
            val s = Bitmap.createScaledBitmap(b, 9, 8, true)
            var h = 0L
            for (y in 0 until 8) for (x in 0 until 8) if (gray(s.getPixel(x, y)) > gray(s.getPixel(x + 1, y))) h = h or (1L shl (y * 8 + x))
            return h
        }
    }
}

/** Analysis results kept between runs, one line per photo: id, modified, faces, keep, blur, hash, text. */
object InfoCache {
    private fun file(ctx: Context) = File(ctx.filesDir, "analysis.tsv")

    fun load(ctx: Context): Map<Long, Info> = runCatching {
        file(ctx).useLines { lines ->
            lines.mapNotNull { l ->
                val f = l.split('\t'); if (f.size != 7) return@mapNotNull null
                f[0].toLong() to Info(f[1].toLong(), f[2].toInt(), f[3] == "1", f[4].toFloat(), f[5].toLong(), f[6].toInt())
            }.toMap()
        }
    }.getOrDefault(emptyMap())

    fun save(ctx: Context, m: Map<Long, Info>) = runCatching {
        file(ctx).bufferedWriter().use { w -> m.forEach { (id, i) -> w.write("$id\t${i.modified}\t${i.faces}\t${if (i.keep) 1 else 0}\t${i.blur}\t${i.hash}\t${i.text}\n") } }
    }
}
