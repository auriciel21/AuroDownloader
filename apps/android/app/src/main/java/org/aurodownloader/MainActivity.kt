package org.aurodownloader

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.media.MediaScannerConnection
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.util.Log
import android.widget.ScrollView
import android.widget.TextView
import org.aurodownloader.engine.EngineClient
import org.aurodownloader.engine.NativeEngine
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * Fenêtre d'essai de la phase 4 (étapes 1 et 2) : démarre le moteur, règle le dossier des
 * téléchargements, s'abonne aux événements et affiche les messages. Essais par adb :
 * `--es url <adresse>` ajoute un téléchargement, `--es dir <dossier>` impose le dossier.
 * L'interface Compose viendra à l'étape 3.
 */
class MainActivity : Activity() {
    private lateinit var log: TextView
    private lateinit var engine: EngineClient

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        log = TextView(this).apply { setTextIsSelectable(true) }
        setContentView(ScrollView(this).apply { addView(log) })

        val failure = NativeEngine.start(this, filesDir.resolve("auro.db").path)
        if (failure.isNotEmpty()) {
            show("Moteur non démarré : $failure")
            Log.e(TAG, "moteur non démarré : $failure")
            return
        }
        engine = EngineClient()
        engine.onNotification { method, params ->
            if (method != "task.progress") show("$method $params")
            if (method == "task.stateChanged" && params.optString("to") == "completed") {
                index(params.optString("taskId"))
            }
        }
        askLegacyStorage()
        chooseFolder(intent.getStringExtra("dir")) {
            engine.call("events.subscribe", JSONObject().put("topics", JSONArray().put("tasks")))
            addFrom(intent)
        }
    }

    // Fenêtre déjà ouverte : la nouvelle demande lui arrive ici.
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (::engine.isInitialized) addFrom(intent)
    }

    override fun onDestroy() {
        if (::engine.isInitialized) engine.close()
        super.onDestroy()
    }

    /**
     * Dossier des téléchargements (décision du 2026-11-08) : `Download/AuroDownloader`,
     * dans le dossier public, écrit par des chemins ordinaires. Réglé au premier démarrage,
     * ou s'il a disparu ; un dossier choisi ensuite n'est pas remplacé. [forced] : essais.
     */
    private fun chooseFolder(forced: String?, then: () -> Unit) {
        engine.call("settings.get") { reply ->
            val values = reply.getOrNull()?.optJSONObject("values")
            val current = values?.takeUnless { it.isNull("downloads.dir") }?.optString("downloads.dir")
            val wanted = when {
                forced != null -> File(forced)
                current.isNullOrEmpty() || !File(current).isDirectory -> defaultFolder()
                else -> null
            }
            if (wanted == null) {
                then()
                return@call
            }
            if (!wanted.mkdirs() && !wanted.isDirectory) {
                show("Dossier impossible à créer : $wanted")
            }
            val set = JSONObject().put("values", JSONObject().put("downloads.dir", wanted.path))
            engine.call("settings.set", set) { result ->
                result.exceptionOrNull()?.let { show("Dossier refusé : ${it.message}") }
                then()
            }
        }
    }

    /**
     * Un fichier écrit par son chemin n'entre pas de lui-même dans l'index des fichiers
     * d'Android (MediaStore) : sans cela, il manque aux recherches et aux fichiers récents
     * de l'application Fichiers. Demandé à la fin de chaque téléchargement.
     */
    private fun index(taskId: String) {
        engine.call("task.get", JSONObject().put("taskId", taskId)) { reply ->
            val task = reply.getOrNull() ?: return@call
            if (task.isNull("fileName")) return@call
            val path = File(task.optString("destDir"), task.optString("fileName")).path
            MediaScannerConnection.scanFile(this, arrayOf(path), null) { scanned, uri ->
                Log.i(TAG, "indexé : $scanned -> $uri")
            }
        }
    }

    private fun defaultFolder(): File =
        File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS), "AuroDownloader")

    /**
     * Android 8 à 10 : écrire dans le dossier public demande la permission d'écriture (et,
     * sous Android 10, `requestLegacyExternalStorage`). Inutile à partir d'Android 11.
     * Pas encore essayé sur un appareil de ces versions.
     */
    private fun askLegacyStorage() {
        if (Build.VERSION.SDK_INT <= Build.VERSION_CODES.Q &&
            checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.WRITE_EXTERNAL_STORAGE), 1)
        }
    }

    private fun addFrom(intent: Intent) {
        val url = intent.getStringExtra("url") ?: return
        val source = JSONObject().put("url", url)
        engine.call("task.add", JSONObject().put("sources", JSONArray().put(source))) { reply ->
            reply.onSuccess { show("Ajoutée : ${it.optString("taskId")}") }
            reply.onFailure { show("Refusée : ${it.message}") }
        }
    }

    private fun show(line: String) {
        Log.i(TAG, line)
        log.append(line)
        log.append("\n")
    }

    companion object {
        const val TAG = "AuroDownloader"
    }
}
