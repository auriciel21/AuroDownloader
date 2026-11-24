package org.aurodownloader.engine

import android.content.Context
import java.util.concurrent.CopyOnWriteArrayList

/**
 * Le moteur, dans le processus de l'application (bibliothèque native `auro_android`,
 * SPEC §4.4). On lui parle JSON-RPC, un message par appel de [send] ; ses réponses et ses
 * notifications arrivent aux écouteurs, une ligne JSON chacune, depuis un fil du moteur.
 */
object NativeEngine {
    init {
        System.loadLibrary("auro_android")
    }

    private val listeners = CopyOnWriteArrayList<(String) -> Unit>()

    /**
     * Initialise la vérification des certificats d'Android puis démarre le moteur sur la
     * base [db] ; rend `""`, ou la raison d'un échec. Sans effet s'il tourne déjà.
     */
    @JvmStatic
    external fun start(context: Context, db: String): String

    /** Un message JSON-RPC pour le moteur. */
    @JvmStatic
    external fun send(message: String)

    fun listen(listener: (String) -> Unit) {
        listeners.add(listener)
    }

    /** Appelée par le moteur pour chaque message ; depuis l'un de ses fils. */
    @JvmStatic
    fun onMessage(message: String) {
        for (listener in listeners) {
            listener(message)
        }
    }
}
