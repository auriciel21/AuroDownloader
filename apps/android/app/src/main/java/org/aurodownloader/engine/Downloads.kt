package org.aurodownloader.engine

import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import org.json.JSONArray
import org.json.JSONObject

/** Un téléchargement tel que l'interface l'affiche (SPEC §6.4). */
data class Task(
    val id: String,
    val name: String,
    val state: String,
    val destDir: String,
    val fileName: String?,
    val doneBytes: Long,
    val totalBytes: Long?,
    val bps: Long = 0,
    val etaS: Long? = null,
    val verifiedBytes: Long? = null,
    val pauseReason: String?,
    val errorCode: String?,
    val errorDetail: String?,
    /** Empreinte attendue : `{algo, value, source}`, ou `null`. */
    val checksum: JSONObject?,
    val createdAt: Long,
    /** `http`, `torrent` ou `media` (SPEC §6.4). */
    val kind: String = "http",
    /** Torrent : octets envoyés aux autres pairs, et débit d'envoi (`task.progress`). */
    val uploadedBytes: Long? = null,
    val uploadBps: Long = 0,
) {
    // Mêmes règles que l'application de bureau (`ui/src/model.js`, SPEC §5.5). `seeding` :
    // arrêter un torrent qui partage le termine (SPEC §7.16), ses fichiers restent.
    val canPause get() = state in setOf("queued", "active", "retry_wait", "verifying", "seeding")
    val canResume get() = state in setOf("paused", "failed")
    val canCancel get() = state in setOf("queued", "active", "paused", "retry_wait", "verifying", "seeding")
    val canRemove get() = state in setOf("completed", "failed", "cancelled")
    val canRestart get() = state in setOf("completed", "failed", "cancelled", "paused", "retry_wait")

    val isTorrent get() = kind == "torrent"

    /** Fichiers d'un torrent à choisir (`task.setFiles`) : pas une fois reçu ni annulé. */
    val canChooseFiles get() = isTorrent && state in setOf("queued", "active", "paused", "retry_wait", "failed")

    /** Un torrent qui partage a fini de recevoir. */
    val isFinished get() = state == "completed" || state == "seeding"

    /** Ratio de partage : octets envoyés sur la taille ; `null` s'il est inconnu. */
    val shareRatio: Double?
        get() = if (uploadedBytes == null || totalBytes == null || totalBytes <= 0) null
        else uploadedBytes.toDouble() / totalBytes

    /** Avancement de 0 à 1 ; `null` si la taille est inconnue. */
    val fraction: Float?
        get() = when {
            isFinished -> 1f
            totalBytes == null || totalBytes <= 0 -> null
            else -> (doneBytes.toDouble() / totalBytes).coerceIn(0.0, 1.0).toFloat()
        }

    companion object {
        fun from(json: JSONObject): Task = Task(
            id = json.getString("taskId"),
            name = json.text("displayName") ?: json.text("fileName") ?: json.getString("taskId"),
            state = json.optString("state"),
            destDir = json.optString("destDir"),
            fileName = json.text("fileName"),
            doneBytes = json.optLong("doneBytes"),
            totalBytes = json.long("totalBytes"),
            pauseReason = json.text("pauseReason"),
            errorCode = json.text("errorCode"),
            errorDetail = json.text("errorDetail"),
            checksum = json.optJSONObject("checksum"),
            createdAt = json.optLong("createdAt"),
            kind = json.text("kind") ?: "http",
        )
    }
}

/** Un fichier d'un torrent (`task.get` avec `files`, SPEC §6.5). */
data class TorrentFile(
    val index: Int,
    /** Chemin dans le torrent, composants séparés par `/`. */
    val path: String,
    val length: Long,
    val selected: Boolean,
    /** Indication seulement (SPEC §7.16). */
    val doneBytes: Long,
) {
    companion object {
        fun from(json: JSONObject) = TorrentFile(
            index = json.optInt("index"),
            path = json.optString("path"),
            length = json.optLong("length"),
            selected = json.optBoolean("selected", true),
            doneBytes = json.optLong("doneBytes"),
        )
    }
}

internal fun JSONObject.text(key: String): String? = if (isNull(key)) null else optString(key)
internal fun JSONObject.long(key: String): Long? = if (isNull(key)) null else optLong(key)

/**
 * Téléchargements du moteur, pour l'interface : un instantané (`sync.snapshot`), puis les
 * notifications plus récentes que sa révision ; une révision sautée fait recharger
 * l'instantané (SPEC §6.10, comme `ui/src/model.js`). Sur le fil principal.
 */
class Downloads(private val engine: EngineClient) {
    val tasks = mutableStateMapOf<String, Task>()
    val settings = mutableStateMapOf<String, Any?>()
    /** Limite de débit globale, en octets par seconde ; `null` : aucune. */
    val downloadLimit = mutableStateOf<Long?>(null)
    /** Limite d'envoi globale des torrents, en octets par seconde ; `null` : aucune. */
    val uploadLimit = mutableStateOf<Long?>(null)
    /** Faux tant que le premier instantané n'est pas arrivé. */
    val loaded = mutableStateOf(false)

    /** Appelé quand une tâche se termine (indexation du fichier). */
    var onCompleted: (Task) -> Unit = {}

    /** Appelé quand une tâche se termine en échec (notification). */
    var onFailed: (Task) -> Unit = {}

    /** Appelé après chaque changement de l'état (arrière-plan, notification). */
    var onChanged: () -> Unit = {}

    private var rev: Long? = null
    private val waiting = mutableListOf<Pair<String, JSONObject>>()

    init {
        engine.onNotification { method, params -> apply(method, params) }
        engine.call("events.subscribe", JSONObject().put("topics", JSONArray().put("tasks").put("queues").put("progress")))
        reload()
    }

    /** Ordre de la liste : les plus récents d'abord. */
    fun list(): List<Task> = tasks.values.sortedWith(compareByDescending<Task> { it.createdAt }.thenBy { it.id })

    fun reload() {
        rev = null
        waiting.clear()
        engine.call("sync.snapshot") { reply -> reply.onSuccess { load(it) } }
    }

    private fun load(snapshot: JSONObject) {
        // Octets envoyés d'un torrent : seulement dans `task.progress` ; gardés d'un
        // instantané à l'autre.
        val uploaded = tasks.mapValues { it.value.uploadedBytes }
        tasks.clear()
        val list = snapshot.optJSONArray("tasks") ?: JSONArray()
        for (i in 0 until list.length()) {
            val task = Task.from(list.getJSONObject(i)).let { it.copy(uploadedBytes = uploaded[it.id]) }
            tasks[task.id] = task
        }
        settings.clear()
        snapshot.optJSONObject("settings")?.let { values ->
            for (key in values.keys()) settings[key] = values.opt(key).takeUnless { it == JSONObject.NULL }
        }
        downloadLimit.value = snapshot.optJSONObject("limits")?.long("downloadBps")
        uploadLimit.value = snapshot.optJSONObject("limits")?.long("uploadBps")
        rev =snapshot.optLong("rev")
        loaded.value = true
        val pending = waiting.toList()
        waiting.clear()
        for ((method, params) in pending) apply(method, params)
        onChanged()
    }

    private fun apply(method: String, params: JSONObject) {
        update(method, params)
        if (rev != null) onChanged()
    }

    private fun update(method: String, params: JSONObject) {
        val current = rev
        if (current == null) {
            waiting.add(method to params)
            return
        }
        if (method == "task.progress") {
            if (params.optLong("rev") < current) return
            val items = params.optJSONArray("items") ?: return
            for (i in 0 until items.length()) {
                val item = items.getJSONObject(i)
                val task = tasks[item.optString("taskId")] ?: continue
                tasks[task.id] = task.copy(
                    doneBytes = item.optLong("doneBytes", task.doneBytes),
                    totalBytes = if (item.has("totalBytes")) item.long("totalBytes") else task.totalBytes,
                    bps = item.optLong("bps"),
                    etaS = item.long("etaS"),
                    verifiedBytes = if (item.has("verifiedBytes")) item.long("verifiedBytes") else task.verifiedBytes,
                    uploadedBytes = if (item.has("uploadedBytes")) item.long("uploadedBytes") else task.uploadedBytes,
                    uploadBps = item.optLong("uploadBps"),
                )
            }
            return
        }
        if (method == "engine.warning") {
            if (params.optString("code") == "EVENTS_LOST") reload()
            return
        }
        if (method !in REVISED) return
        val at = params.optLong("rev")
        if (at <= current) return
        if (at > current + 1) {
            reload()
            return
        }
        rev = at
        when (method) {
            "task.added" -> params.optJSONObject("task")?.let { Task.from(it) }?.let { tasks[it.id] = it }
            "task.stateChanged" -> {
                val task = tasks[params.optString("taskId")] ?: return reload()
                val to = params.optString("to")
                var next = task.copy(
                    state = to,
                    pauseReason = params.text("pauseReason"),
                    errorCode = params.text("errorCode"),
                )
                if (to != "active") next = next.copy(bps = 0, etaS = null)
                if (to != "verifying") next = next.copy(verifiedBytes = null)
                if (next.isFinished && (next.totalBytes ?: 0) > 0) next = next.copy(doneBytes = next.totalBytes!!)
                if (to != "seeding") next = next.copy(uploadBps = 0)
                tasks[task.id] = next
                // Fichiers à leur nom final : à la fin d'un téléchargement, ou dès le partage
                // d'un torrent (SPEC §7.16) ; pas une seconde fois quand le partage s'arrête.
                if (to == "seeding" || (to == "completed" && task.state != "seeding")) onCompleted(next)
                if (to == "failed") onFailed(next)
            }
            "task.updated" -> {
                val task = tasks[params.optString("taskId")] ?: return reload()
                var next = task
                if (params.has("displayName")) next = next.copy(name = params.text("displayName") ?: next.name)
                if (params.has("fileName")) next = next.copy(fileName = params.text("fileName"))
                if (params.has("destDir")) next = next.copy(destDir = params.optString("destDir"))
                if (params.has("totalBytes")) next = next.copy(totalBytes = params.long("totalBytes"))
                if (params.has("doneBytes")) next = next.copy(doneBytes = params.optLong("doneBytes"))
                if (params.has("errorCode")) next = next.copy(errorCode = params.text("errorCode"))
                if (params.has("errorDetail")) next = next.copy(errorDetail = params.text("errorDetail"))
                if (params.has("checksum")) next = next.copy(checksum = params.optJSONObject("checksum"))
                tasks[task.id] = next
            }
            "task.removed" -> tasks.remove(params.optString("taskId"))
            "settings.updated" -> params.optJSONObject("values")?.let { values ->
                for (key in values.keys()) settings[key] = values.opt(key).takeUnless { it == JSONObject.NULL }
            }
            "limits.updated" -> {
                if (params.has("downloadBps")) downloadLimit.value = params.long("downloadBps")
                if (params.has("uploadBps")) uploadLimit.value = params.long("uploadBps")
            }
        }
    }

    companion object {
        /** Notifications numérotées : chacune avance la révision d'un cran. */
        private val REVISED = setOf(
            "task.added", "task.stateChanged", "task.updated", "task.removed", "queue.updated",
            "limits.updated", "settings.updated", "schedule.updated", "schedule.removed",
            "engine.endAction", "engine.network",
        )
    }
}
