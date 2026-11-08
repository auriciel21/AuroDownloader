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
) {
    val canPause get() = state in setOf("queued", "active", "retry_wait", "verifying")
    val canResume get() = state in setOf("paused", "failed")
    val canCancel get() = state in setOf("queued", "active", "paused", "retry_wait", "verifying")
    val canRemove get() = state in setOf("completed", "failed", "cancelled")
    val canRestart get() = state in setOf("completed", "failed", "cancelled", "paused", "retry_wait")

    /** Avancement de 0 à 1 ; `null` si la taille est inconnue. */
    val fraction: Float?
        get() = when {
            state == "completed" -> 1f
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
    /** Faux tant que le premier instantané n'est pas arrivé. */
    val loaded = mutableStateOf(false)

    /** Appelé quand une tâche se termine (indexation du fichier). */
    var onCompleted: (Task) -> Unit = {}

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
        tasks.clear()
        val list = snapshot.optJSONArray("tasks") ?: JSONArray()
        for (i in 0 until list.length()) {
            val task = Task.from(list.getJSONObject(i))
            tasks[task.id] = task
        }
        settings.clear()
        snapshot.optJSONObject("settings")?.let { values ->
            for (key in values.keys()) settings[key] = values.opt(key).takeUnless { it == JSONObject.NULL }
        }
        downloadLimit.value = snapshot.optJSONObject("limits")?.long("downloadBps")
        rev = snapshot.optLong("rev")
        loaded.value = true
        val pending = waiting.toList()
        waiting.clear()
        for ((method, params) in pending) apply(method, params)
    }

    private fun apply(method: String, params: JSONObject) {
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
                if (to == "completed" && (next.totalBytes ?: 0) > 0) next = next.copy(doneBytes = next.totalBytes!!)
                tasks[task.id] = next
                if (to == "completed") onCompleted(next)
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
            "limits.updated" -> downloadLimit.value = params.long("downloadBps")
        }
    }

    companion object {
        /** Notifications numérotées : chacune avance la révision d'un cran. */
        private val REVISED = setOf(
            "task.added", "task.stateChanged", "task.updated", "task.removed", "queue.updated",
            "limits.updated", "settings.updated", "schedule.updated", "schedule.removed",
            "engine.endAction",
        )
    }
}
