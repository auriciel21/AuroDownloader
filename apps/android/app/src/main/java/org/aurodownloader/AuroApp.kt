package org.aurodownloader

import android.app.Application
import android.media.MediaScannerConnection
import android.os.Environment
import android.util.Log
import org.aurodownloader.engine.Downloads
import org.aurodownloader.engine.EngineClient
import org.aurodownloader.engine.NativeEngine
import org.aurodownloader.engine.Task
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * Le processus de l'application : le moteur y démarre une fois (SPEC §4.4), avec un
 * client et l'état des téléchargements partagés par l'interface et, plus tard, le
 * service d'arrière-plan.
 */
class AuroApp : Application() {
    /** Raison d'un échec du démarrage du moteur ; `null` s'il tourne. */
    var failure: String? = null
        private set
    lateinit var engine: EngineClient
        private set
    lateinit var downloads: Downloads
        private set

    override fun onCreate() {
        super.onCreate()
        val reason = NativeEngine.start(this, filesDir.resolve("auro.db").path)
        if (reason.isNotEmpty()) {
            failure = reason
            Log.e(TAG, "moteur non démarré : $reason")
            return
        }
        engine = EngineClient()
        downloads = Downloads(engine)
        downloads.onCompleted = { task -> index(task) }
        chooseFolder()
    }

    val running get() = failure == null

    // ---------- Actions ----------

    /** Ajoute un téléchargement ; [done] reçoit l'erreur du moteur, ou `null`. */
    fun add(url: String, done: (Throwable?) -> Unit = {}) {
        val source = JSONObject().put("url", url.trim())
        engine.call("task.add", JSONObject().put("sources", JSONArray().put(source))) {
            done(it.exceptionOrNull())
        }
    }

    fun act(method: String, taskId: String, extra: JSONObject = JSONObject(), done: (Throwable?) -> Unit = {}) {
        extra.put("taskIds", JSONArray().put(taskId))
        engine.call(method, extra) { reply ->
            // Refus d'une tâche : dans `errors` de la réponse (SPEC §6.4).
            val error = reply.exceptionOrNull() ?: reply.getOrNull()?.optJSONArray("errors")
                ?.optJSONObject(0)?.let { Exception(it.optString("message")) }
            done(error)
        }
    }

    fun setLimit(bps: Long?, done: (Throwable?) -> Unit = {}) {
        engine.call("limits.set", JSONObject().put("downloadBps", bps ?: JSONObject.NULL)) {
            done(it.exceptionOrNull())
        }
    }

    fun setSetting(key: String, value: Any?, done: (Throwable?) -> Unit = {}) {
        val values = JSONObject().put(key, value ?: JSONObject.NULL)
        engine.call("settings.set", JSONObject().put("values", values)) { done(it.exceptionOrNull()) }
    }

    // ---------- Dossier et index ----------

    /**
     * Dossier des téléchargements (décision du 2026-11-08) : `Download/AuroDownloader` dans
     * le dossier public, écrit par des chemins ordinaires. Réglé au premier démarrage, ou
     * s'il a disparu ; un dossier choisi ensuite n'est pas remplacé.
     */
    private fun chooseFolder() {
        engine.call("settings.get") { reply ->
            val values = reply.getOrNull()?.optJSONObject("values") ?: return@call
            val current = if (values.isNull("downloads.dir")) null else values.optString("downloads.dir")
            if (!current.isNullOrEmpty() && File(current).isDirectory) return@call
            val wanted = defaultFolder()
            if (!wanted.mkdirs() && !wanted.isDirectory) {
                Log.e(TAG, "dossier impossible à créer : $wanted")
            }
            setSetting("downloads.dir", wanted.path)
        }
    }

    fun defaultFolder(): File =
        File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS), "AuroDownloader")

    /**
     * Un fichier écrit par son chemin n'entre pas de lui-même dans l'index des fichiers
     * d'Android (MediaStore) : sans cela, il manque aux recherches et aux fichiers récents
     * de l'application Fichiers.
     */
    private fun index(task: Task) {
        val name = task.fileName ?: return
        val path = File(task.destDir, name).path
        MediaScannerConnection.scanFile(this, arrayOf(path), null) { _, uri ->
            Log.i(TAG, "indexé : $path -> $uri")
        }
    }

    companion object {
        const val TAG = "AuroDownloader"
    }
}
