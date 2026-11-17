package org.aurodownloader

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.widget.Toast

/**
 * Reçoit un lien partagé depuis une autre application (« Partager ») ou ouvert avec elle,
 * l'ajoute, ouvre la liste, puis se ferme. Activité à part, sans fenêtre ni place dans
 * les applications récentes : elle démarre à neuf à chaque lien. Confiés à la fenêtre
 * principale, ces liens se perdaient quand Android la recréait depuis son état sauvegardé
 * (processus arrêté, fenêtre restée dans les récentes) : il lui rend la demande d'origine.
 *
 * Étape 5e (SPEC §7.16, décision du 2026-11-10) : liens magnet et fichiers `.torrent`
 * (partagés, ouverts depuis Fichiers ou le navigateur) s'ajoutent aussi d'office ; le choix
 * des fichiers se fait ensuite dans les détails.
 */
class ShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val app = application as AuroApp
        if (app.running) {
            val report: (Throwable?) -> Unit = { error ->
                val text = if (error == null) app.getString(R.string.added) else app.getString(R.string.refused, error.message ?: "")
                Toast.makeText(app, text, Toast.LENGTH_LONG).show()
            }
            when (val received = receivedOf(intent)) {
                is Received.Link -> app.add(received.url, report)
                is Received.TorrentFile -> app.readTorrent(received.uri) { read ->
                    read.fold({ app.addTorrent(it, report) }, report)
                }
                null -> {}
            }
        }
        // La liste à l'écran : elle demande aussi à Android de protéger le téléchargement.
        startActivity(Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        finish()
    }

    /** Ce qu'une autre application confie : une adresse (http, https, magnet) ou un `.torrent`. */
    sealed interface Received {
        data class Link(val url: String) : Received
        data class TorrentFile(val uri: Uri) : Received
    }

    companion object {
        private val LINK = Regex("""(?:(?:https?|ftps?|sftp)://|magnet:\?)\S+""", RegexOption.IGNORE_CASE)
        const val TORRENT_TYPE = "application/x-bittorrent"

        /**
         * Lien reçu : texte partagé (première adresse http(s), ftp(s) ou sftp, ou lien magnet), fichier
         * partagé ou ouvert (`.torrent`), ou lien ouvert. Une adresse http(s) d'un `.torrent`
         * (type annoncé, ou chemin en `.torrent`) est lue comme un torrent, pas téléchargée
         * comme un fichier.
         */
        fun receivedOf(intent: Intent): Received? = when (intent.action) {
            Intent.ACTION_SEND -> streamOf(intent)?.let { Received.TorrentFile(it) }
                ?: intent.getStringExtra(Intent.EXTRA_TEXT)?.let { LINK.find(it)?.value }?.let { Received.Link(it) }
            Intent.ACTION_VIEW -> intent.data?.let { uri ->
                when (uri.scheme?.lowercase()) {
                    "magnet" -> Received.Link(uri.toString())
                    "ftp", "ftps", "sftp" -> Received.Link(uri.toString())
                    "http", "https" -> if (isTorrent(intent.type, uri)) Received.TorrentFile(uri) else Received.Link(uri.toString())
                    "content", "file" -> Received.TorrentFile(uri)
                    else -> null
                }
            }
            else -> null
        }

        private fun isTorrent(type: String?, uri: Uri) =
            type.equals(TORRENT_TYPE, ignoreCase = true) || uri.path?.endsWith(".torrent", ignoreCase = true) == true

        private fun streamOf(intent: Intent): Uri? =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                @Suppress("DEPRECATION")
                intent.getParcelableExtra(Intent.EXTRA_STREAM)
            }
    }
}
