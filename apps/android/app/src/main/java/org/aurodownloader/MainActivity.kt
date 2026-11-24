package org.aurodownloader

import android.Manifest
import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.webkit.MimeTypeMap
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.core.content.FileProvider
import org.aurodownloader.engine.Task
import org.aurodownloader.ui.AuroScreens
import org.aurodownloader.ui.AuroTheme
import java.io.File

/**
 * Fenêtre de l'application (SPEC §4.4, F-38) : la liste des téléchargements, leurs détails
 * et les réglages (Compose). Ajoute aussi un lien partagé depuis une autre application
 * (« Partager »), ouvert avec elle, ou donné par adb (`--es url <adresse>`, essais : APK de
 * débogage seulement).
 */
class MainActivity : ComponentActivity() {
    private val app get() = application as AuroApp

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        askLegacyStorage()
        askNotifications()
        if (savedInstanceState == null) addFrom(intent)
        setContent {
            AuroTheme {
                AuroScreens(app, open = ::openFile)
            }
        }
    }

    // À l'écran, l'application protège elle-même ses téléchargements ; elle demande aussi la
    // protection d'Android pour la suite ([Background]).
    override fun onStart() {
        super.onStart()
        if (app.running) app.background.visible = true
    }

    override fun onStop() {
        if (app.running) app.background.visible = false
        super.onStop()
    }

    // Fenêtre déjà ouverte : la nouvelle demande lui arrive ici.
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        addFrom(intent)
    }

    /**
     * Adresse donnée par adb (`--es url`, essais) ; les liens partagés ou ouverts passent par
     * [ShareActivity]. Seulement dans l'APK de débogage : l'activité est ouverte à toutes les
     * applications, qui pourraient sinon ajouter un téléchargement sans rien montrer (audit C15).
     */
    private fun addFrom(intent: Intent) {
        if (!debuggable()) return
        if (!app.running) return
        val url = intent.getStringExtra("url") ?: return
        app.add(url) { error ->
            val text = if (error == null) getString(R.string.added) else getString(R.string.refused, error.message ?: "")
            Toast.makeText(this, text, Toast.LENGTH_LONG).show()
        }
    }

    /** APK de débogage (essais), jamais celui de publication. */
    private fun debuggable() = applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0

    /**
     * Ouvre un fichier terminé avec l'application qui sait le lire ; jamais un programme ni
     * un paquet d'installation (SPEC §8.1).
     */
    private fun openFile(task: Task) {
        val name = task.fileName ?: return
        val extension = name.substringAfterLast('.', "").lowercase()
        if (extension in EXECUTABLE) {
            Toast.makeText(this, R.string.not_opened_program, Toast.LENGTH_LONG).show()
            return
        }
        val uri = FileProvider.getUriForFile(this, "$packageName.files", File(task.destDir, name))
        val type = MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension) ?: "application/octet-stream"
        val view = Intent(Intent.ACTION_VIEW).setDataAndType(uri, type).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        try {
            startActivity(Intent.createChooser(view, name))
        } catch (e: ActivityNotFoundException) {
            Toast.makeText(this, R.string.no_app_to_open, Toast.LENGTH_LONG).show()
        }
    }

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

    /** Android 13 et plus : la notification de progression demande la permission. */
    private fun askNotifications() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 2)
        }
    }

    companion object {
        /** Programmes et paquets d'installation : jamais ouverts depuis l'application. */
        private val EXECUTABLE = setOf("apk", "apks", "xapk", "apkm", "aab", "exe", "msi", "bat", "cmd", "sh", "jar")
    }
}
