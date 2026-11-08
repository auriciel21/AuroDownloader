package org.aurodownloader

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.os.Environment
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.widget.ScrollView
import android.widget.TextView
import org.aurodownloader.engine.NativeEngine
import org.json.JSONArray
import org.json.JSONObject

/**
 * Première fenêtre, pour l'étape 1 de la phase 4 : démarre le moteur, s'abonne à ses
 * événements et affiche ses messages. Une adresse donnée par `adb shell am start … --es url
 * <adresse>` est ajoutée au démarrage (essais). L'interface Compose viendra à l'étape 3.
 */
class MainActivity : Activity() {
    private val main = Handler(Looper.getMainLooper())
    private lateinit var log: TextView
    private var nextId = 1

    private val listener: (String) -> Unit = { message ->
        Log.i(TAG, message)
        main.post { show(message) }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        log = TextView(this).apply { setTextIsSelectable(true) }
        setContentView(ScrollView(this).apply { addView(log) })

        NativeEngine.listen(listener)
        val failure = NativeEngine.start(this, filesDir.resolve("auro.db").path)
        if (failure.isNotEmpty()) {
            show("Moteur non démarré : $failure")
            Log.e(TAG, "moteur non démarré : $failure")
            return
        }
        // Dossier des téléchargements : celui de l'application, en attendant le sélecteur
        // de stockage (étape 2).
        val dir = getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: filesDir
        dir.mkdirs()
        call("settings.set", JSONObject().put("values", JSONObject().put("downloads.dir", dir.path)))
        call("events.subscribe", JSONObject().put("topics", JSONArray().put("tasks")))
        addFrom(intent)
    }

    // Fenêtre déjà ouverte : la nouvelle demande lui arrive ici.
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        addFrom(intent)
    }

    private fun addFrom(intent: Intent) {
        intent.getStringExtra("url")?.let { url ->
            val source = JSONObject().put("url", url)
            call("task.add", JSONObject().put("sources", JSONArray().put(source)))
        }
    }

    override fun onDestroy() {
        NativeEngine.forget(listener)
        super.onDestroy()
    }

    private fun call(method: String, params: JSONObject) {
        val message = JSONObject()
            .put("jsonrpc", "2.0")
            .put("id", nextId++)
            .put("method", method)
            .put("params", params)
        NativeEngine.send(message.toString())
    }

    private fun show(line: String) {
        log.append(line)
        log.append("\n")
    }

    companion object {
        const val TAG = "AuroDownloader"
    }
}
