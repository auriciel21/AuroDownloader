package org.aurodownloader

import android.app.Application
import android.media.MediaScannerConnection
import android.net.Uri
import android.os.Environment
import android.os.Handler
import android.os.Looper
import android.util.Base64
import android.util.Log
import org.aurodownloader.engine.Downloads
import org.aurodownloader.engine.EngineClient
import org.aurodownloader.engine.NativeEngine
import org.aurodownloader.engine.Task
import org.aurodownloader.engine.TorrentFile
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

    /** Ajoute un fichier `.torrent` (son contenu, envoyé en base64, SPEC §7.16). */
    fun addTorrent(bytes: ByteArray, done: (Throwable?) -> Unit = {}) {
        val source = JSONObject().put("torrent", Base64.encodeToString(bytes, Base64.NO_WRAP))
        afterFolder {
            engine.call("task.add", JSONObject().put("sources", JSONArray().put(source))) {
                done(it.exceptionOrNull())
            }
        }
    }

    /**
     * Lit un fichier `.torrent` choisi ou ouvert (`content:` ou `file:`) hors du fil
     * principal ; une adresse `http(s)` est lue par le moteur (`torrent.fetch`), qui suit son
     * proxy et « Wi-Fi seulement » (audit C16). [done] reçoit son contenu, sur le fil
     * principal. Refusé au-delà de 16 Mio, ou s'il ne commence pas comme un dictionnaire
     * bencode (une page d'erreur, par exemple).
     */
    fun readTorrent(uri: Uri, done: (Result<ByteArray>) -> Unit) {
        if (uri.scheme == "http" || uri.scheme == "https") {
            engine.call("torrent.fetch", JSONObject().put("url", uri.toString())) { reply ->
                done(reply.mapCatching { Base64.decode(it.getString("data"), Base64.DEFAULT) })
            }
            return
        }
        Thread {
            val result = runCatching {
                val input = contentResolver.openInputStream(uri) ?: error(getString(R.string.torrent_unreadable))
                val bytes = input.use { stream ->
                    val out = java.io.ByteArrayOutputStream()
                    val buffer = ByteArray(64 * 1024)
                    while (true) {
                        val n = stream.read(buffer)
                        if (n < 0) break
                        out.write(buffer, 0, n)
                        if (out.size() > MAX_TORRENT_FILE) error(getString(R.string.torrent_too_big))
                    }
                    out.toByteArray()
                }
                if (bytes.isEmpty() || bytes[0] != 'd'.code.toByte()) error(getString(R.string.torrent_invalid))
                bytes
            }
            main.post { done(result) }
        }.start()
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

    /** Limite d'envoi des torrents ; `null` : aucune. */
    fun setUploadLimit(bps: Long?, done: (Throwable?) -> Unit = {}) {
        engine.call("limits.set", JSONObject().put("uploadBps", bps ?: JSONObject.NULL)) {
            done(it.exceptionOrNull())
        }
    }

    /** Fichiers d'un torrent (`task.get` avec `files`) ; [done] reçoit la liste. */
    fun files(taskId: String, done: (Result<List<TorrentFile>>) -> Unit) {
        val params = JSONObject().put("taskId", taskId).put("include", JSONArray().put("files"))
        engine.call("task.get", params) { reply ->
            done(reply.map { task ->
                val list = task.optJSONArray("files") ?: JSONArray()
                (0 until list.length()).map { TorrentFile.from(list.getJSONObject(it)) }
            })
        }
    }

    /** Choisit les fichiers d'un torrent à télécharger (`task.setFiles`, rangs). */
    fun setFiles(taskId: String, selected: List<Int>, done: (Throwable?) -> Unit = {}) {
        val ranks = JSONArray()
        selected.forEach { ranks.put(it) }
        engine.call("task.setFiles", JSONObject().put("taskId", taskId).put("selected", ranks)) {
            done(it.exceptionOrNull())
        }
    }

    fun setSetting(key: String, value: Any?, done: (Throwable?) -> Unit = {}) {
        val values = JSONObject().put(key, value ?: JSONObject.NULL)
        engine.call("settings.set", JSONObject().put("values", values)) { done(it.exceptionOrNull()) }
    }

    // ---------- Clés des serveurs SSH (SFTP, SPEC §7.22, étape 6d) ----------

    /** Clé d'un serveur SSH vue par le moteur, en attente de la décision de l'utilisateur. */
    data class PendingKey(
        val host: String,
        val port: Int,
        val algorithm: String,
        val fingerprint: String,
        val changed: Boolean,
    )

    /**
     * Clé en attente du serveur de la tâche `taskId` (adresse `sftp://`), ou `null` : autre
     * adresse, ou clé plus en attente (le moteur ne la garde qu'en mémoire).
     */
    fun pendingKey(taskId: String, done: (PendingKey?) -> Unit) {
        val params = JSONObject().put("taskId", taskId).put("include", JSONArray().put("sources"))
        engine.call("task.get", params) { reply ->
            val url = reply.getOrNull()?.optJSONArray("sources")?.optJSONObject(0)?.optString("url")
            val uri = url?.let { runCatching { android.net.Uri.parse(it) }.getOrNull() }
            val name = uri?.host
            if (uri?.scheme?.lowercase() != "sftp" || name.isNullOrEmpty()) {
                done(null)
                return@call
            }
            val host = name.trim('[', ']').lowercase()
            val port = if (uri.port > 0) uri.port else 22
            engine.call("hostkey.list") { keys ->
                val pending = keys.getOrNull()?.optJSONArray("pending") ?: JSONArray()
                val found = (0 until pending.length()).map { pending.getJSONObject(it) }
                    .firstOrNull { it.optString("host") == host && it.optInt("port") == port }
                done(found?.let {
                    PendingKey(host, port, it.optString("algorithm"), it.optString("fingerprint"), it.optBoolean("changed"))
                })
            }
        }
    }

    /** Accepte la clé ; le moteur relance les tâches qu'elle bloquait. */
    fun acceptKey(key: PendingKey, done: (Throwable?) -> Unit = {}) {
        val params = JSONObject().put("host", key.host).put("port", key.port).put("fingerprint", key.fingerprint)
        engine.call("hostkey.accept", params) { done(it.exceptionOrNull()) }
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
        val top = File(task.destDir, name)
        // Torrent à plusieurs fichiers : un dossier, dont chaque fichier s'indexe.
        val paths = if (top.isDirectory) {
            top.walkTopDown().filter { it.isFile && !it.name.endsWith(".part") }.map { it.path }.toList()
        } else {
            listOf(top.path)
        }
        if (paths.isEmpty()) return
        // Jamais de chemin dans le journal du système (lisible par adb, rapports de bogue) :
        // l'identifiant de la tâche seulement (audit C25).
        MediaScannerConnection.scanFile(this, paths.toTypedArray(), null) { _, uri ->
            Log.i(TAG, "indexé : tâche ${task.id}, ${if (uri != null) "ok" else "échec"}")
        }
    }

    private val main = Handler(Looper.getMainLooper())

    companion object {
        const val TAG = "AuroDownloader"

        /** Plus gros fichier `.torrent` lu (comme l'application de bureau, SPEC §4.6). */
        const val MAX_TORRENT_FILE = 16 * 1024 * 1024
    }
}
