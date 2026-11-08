package org.aurodownloader

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.widget.Toast

/**
 * Reçoit un lien partagé depuis une autre application (« Partager ») ou ouvert avec elle,
 * l'ajoute, ouvre la liste, puis se ferme. Activité à part, sans fenêtre ni place dans
 * les applications récentes : elle démarre à neuf à chaque lien. Confiés à la fenêtre
 * principale, ces liens se perdaient quand Android la recréait depuis son état sauvegardé
 * (processus arrêté, fenêtre restée dans les récentes) : il lui rend la demande d'origine.
 */
class ShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val app = application as AuroApp
        val url = linkOf(intent)
        if (url != null && app.running) {
            app.add(url) { error ->
                val text = if (error == null) getString(R.string.added) else getString(R.string.refused, error.message ?: "")
                Toast.makeText(app, text, Toast.LENGTH_LONG).show()
            }
        }
        // La liste à l'écran : elle demande aussi à Android de protéger le téléchargement.
        startActivity(Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        finish()
    }

    companion object {
        private val LINK = Regex("""https?://\S+""", RegexOption.IGNORE_CASE)

        /** Lien reçu : texte partagé (première adresse http(s)), ou lien ouvert. */
        fun linkOf(intent: Intent): String? = when (intent.action) {
            Intent.ACTION_SEND -> intent.getStringExtra(Intent.EXTRA_TEXT)?.let { LINK.find(it)?.value }
            Intent.ACTION_VIEW -> intent.data?.takeIf { it.scheme == "http" || it.scheme == "https" }?.toString()
            else -> null
        }
    }
}
