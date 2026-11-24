package org.aurodownloader.engine

import android.os.Handler
import android.os.Looper
import org.json.JSONObject
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

/**
 * Client JSON-RPC du moteur dans le processus ([NativeEngine]) : chaque réponse revient à
 * la demande qui l'attend, chaque notification aux écouteurs ; les deux sur le fil
 * principal.
 */
class EngineClient {
    private val main = Handler(Looper.getMainLooper())
    private val nextId = AtomicLong(1)
    private val pending = ConcurrentHashMap<Long, (Result<JSONObject>) -> Unit>()
    private val notifications = mutableListOf<(method: String, params: JSONObject) -> Unit>()

    private val listener: (String) -> Unit = { line -> main.post { dispatch(line) } }

    init {
        NativeEngine.listen(listener)
    }

    /** Appelle [method] ; [onReply] reçoit `result`, ou l'erreur du moteur. */
    fun call(method: String, params: JSONObject = JSONObject(), onReply: (Result<JSONObject>) -> Unit = {}) {
        val id = nextId.getAndIncrement()
        pending[id] = onReply
        val message = JSONObject()
            .put("jsonrpc", "2.0")
            .put("id", id)
            .put("method", method)
            .put("params", params)
        NativeEngine.send(message.toString())
    }

    fun onNotification(listener: (method: String, params: JSONObject) -> Unit) {
        notifications.add(listener)
    }

    private fun dispatch(line: String) {
        val message = try {
            JSONObject(line)
        } catch (e: org.json.JSONException) {
            return
        }
        if (message.has("id") && !message.isNull("id")) {
            val reply = pending.remove(message.getLong("id")) ?: return
            val error = message.optJSONObject("error")
            if (error != null) {
                reply(Result.failure(EngineError(error.optString("message"), error.optJSONObject("data"))))
            } else {
                reply(Result.success(message.optJSONObject("result") ?: JSONObject()))
            }
        } else {
            val method = message.optString("method")
            val params = message.optJSONObject("params") ?: JSONObject()
            for (listener in notifications) {
                listener(method, params)
            }
        }
    }
}

/** Refus du moteur ; `data.code` porte le code stable (SPEC §6.8). */
class EngineError(message: String, val data: JSONObject?) : Exception(message) {
    val code: String? get() = data?.optString("code")
}
