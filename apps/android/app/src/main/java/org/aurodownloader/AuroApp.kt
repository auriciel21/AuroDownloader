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
    lateinit var background: Background
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
        background = Background(this)
        downloads.onCompleted = { task ->
            index(task)
            if (!background.visible) background.notifyEnd(task, failed = false)
        }
        chooseFolder()
    }

    val running get() = failure == null

    // ---------- Actions ----------

    /**
     * Ajoute un téléchargement ; [done] reçoit l'erreur du moteur, ou `null`. Attend que le
     * dossier soit réglé ([chooseFolder]) : avant, le moteur n'a pas de dossier où écrire.
     */
    fun add(url: String, done: (Throwable?) -> Unit = {}) {
        val source = JSONObject().put("url", url.trim())
        afterFolder {
            engine.call("task.add", JSONObject().put("sources", JSONArray().put(source))) {
                done(it.exceptionOrNull())
            }
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
     *
     * Le moteur n'a pas de dossier « Téléchargements » du système à prendre par défaut sous
     * Android : un ajout arrivé avant ce réglage serait refusé (premier démarrage par un
     * lien partagé, essai du 2026-11-09). Les ajouts attendent donc la fin de ce réglage
     * ([afterFolder]), réussi ou non.
     */
    private fun chooseFolder() {
        engine.call("settings.get") { reply ->
            val values = reply.getOrNull()?.optJSONObject("values")
            val current = values?.takeUnless { it.isNull("downloads.dir") }?.optString("downloads.dir")
            if (values == null || (!current.isNullOrEmpty() && File(current).isDirectory)) {
                folderSettled()
                return@call
            }
            val wanted = defaultFolder()
            if (!wanted.mkdirs() && !wanted.isDirectory) {
                Log.e(TAG, "dossier impossible à créer : $wanted")
            }
            setSetting("downloads.dir", wanted.path) { folderSettled() }
        }
    }

    // Fil principal seulement : les réponses du moteur y arrivent, et les ajouts en partent.
    private var folderReady = false
    private val waitingForFolder = mutableListOf<() -> Unit>()

    private fun afterFolder(action: () -> Unit) {
        if (folderReady) action() else waitingForFolder += action
    }

    private fun folderSettled() {
        folderReady = true
        val waiting = waitingForFolder.toList()
        waitingForFolder.clear()
        waiting.forEach { it() }
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
