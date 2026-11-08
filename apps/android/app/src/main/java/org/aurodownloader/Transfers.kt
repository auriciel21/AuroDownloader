package org.aurodownloader

import android.app.Notification
import android.app.Service
import android.app.job.JobParameters
import android.app.job.JobService
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder

/**
 * Transfert lancé par l'utilisateur (Android 14 et plus) : tant qu'il tourne, Android
 * protège le processus, donc le moteur qui télécharge ([Background]). Il ne fait rien
 * lui-même : le moteur télécharge, [Background] tient la notification et le termine.
 */
class TransferJob : JobService() {
    private val background get() = (application as AuroApp).background

    override fun onStartJob(params: JobParameters): Boolean {
        val app = application as AuroApp
        if (!app.running) return false
        background.attach(this, params)
        return true
    }

    /** Arrêté par Android (réseau perdu, surchauffe) : repris plus tard s'il reste du travail. */
    override fun onStopJob(params: JobParameters): Boolean = background.detach(params)
}

/**
 * Service au premier plan de type « synchronisation de données » (Android 8 à 13), même
 * rôle que [TransferJob]. Pas encore essayé sur un appareil de ces versions.
 */
class TransferService : Service() {
    private val background get() = (application as AuroApp).background

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val app = application as AuroApp
        if (!app.running) {
            stopSelf()
            return START_NOT_STICKY
        }
        background.attach(this)
        return START_NOT_STICKY
    }

    fun startInForeground(id: Int, notification: Notification) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(id, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(id, notification)
        }
    }

    fun finish() {
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    /** Android 15 et plus : limite de 6 heures par jour de ce type de service (non utilisé là). */
    override fun onTimeout(startId: Int, fgsType: Int) {
        finish()
        background.detach(this)
    }

    override fun onDestroy() {
        background.detach(this)
        super.onDestroy()
    }
}

/** Bouton « Tout mettre en pause » de la notification. */
class ActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val app = context.applicationContext as AuroApp
        if (intent.action == PAUSE_ALL && app.running) app.background.pauseAll()
    }

    companion object {
        const val PAUSE_ALL = "org.aurodownloader.PAUSE_ALL"
    }
}

/** Redémarrage de l'appareil : voir [Background.onBoot]. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED) return
        val app = context.applicationContext as AuroApp
        if (app.running) app.background.onBoot()
    }
}
