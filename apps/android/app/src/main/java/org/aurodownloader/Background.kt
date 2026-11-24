package org.aurodownloader

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import androidx.compose.runtime.mutableStateOf
import org.aurodownloader.engine.Task
import org.aurodownloader.ui.bytes
import org.aurodownloader.ui.rate
import org.json.JSONArray
import org.json.JSONObject

/**
 * Téléchargements en arrière-plan (SPEC §4.4 et §7.15, phase 4, étape 4).
 *
 * Le moteur ne télécharge que si le réseau convient (connecté ; sans frais au volume avec
 * « Wi-Fi seulement ») et si Android le protège : un transfert lancé par l'utilisateur à
 * partir d'Android 14 ([TransferJob]), un service au premier plan avant ([TransferService]),
 * ou l'application à l'écran. Sinon il le dit au moteur (`engine.setNetwork`), qui remet
 * les tâches en file : un processus qu'Android peut tuer à tout moment ne télécharge pas
 * sans protection, et rien ne part au redémarrage de l'appareil sans l'utilisateur.
 *
 * Sur le fil principal.
 */
class Background(private val app: AuroApp) {
    private val main = Handler(Looper.getMainLooper())
    private val prefs = app.getSharedPreferences("background", Context.MODE_PRIVATE)
    private val notifications = app.getSystemService(NotificationManager::class.java)

    /** L'application est à l'écran. */
    var visible = false
        set(value) {
            field = value
            refresh()
        }

    private var connected = false
    private var unmetered = false
    /** Wi-Fi ou Ethernet, facturé ou non : ce qu'exige « Wi-Fi seulement » (audit C17). */
    private var wifi = false
    private var job: Pair<JobService, JobParameters>? = null
    private var service: TransferService? = null
    private var told: Pair<Boolean, Boolean>? = null
    private var lastNotice = 0L
    private var bootPending = false

    /** Cause de l'attente, pour l'interface : `null` si les téléchargements peuvent partir. */
    val waiting = mutableStateOf<Int?>(null)

    var wifiOnly: Boolean
        get() = prefs.getBoolean("wifiOnly", false)
        set(value) {
            prefs.edit().putBoolean("wifiOnly", value).apply()
            // Un transfert en cours garde sa contrainte de réseau : il est remplacé.
            rescheduleJob()
            refresh()
        }

    var resumeAfterBoot: Boolean
        get() = prefs.getBoolean("resumeAfterBoot", false)
        set(value) {
            prefs.edit().putBoolean("resumeAfterBoot", value).apply()
        }

    init {
        createChannels()
        val connectivity = app.getSystemService(ConnectivityManager::class.java)
        connectivity.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
                main.post {
                    connected = caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
                    unmetered = caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
                    wifi = isWifi(caps)
                    refresh()
                }
            }

            override fun onLost(network: Network) {
                main.post {
                    connected = false
                    unmetered = false
                    wifi = false
                    refresh()
                }
            }
        })
        app.downloads.onChanged = { refresh() }
        app.downloads.onFailed = { task -> if (!visible) notifyEnd(task, failed = true) }
    }

    /** Tâches qui demandent du réseau ou du travail : en attente, en cours, à relancer. */
    private fun busy(): List<Task> =
        app.downloads.tasks.values.filter { it.state in setOf("queued", "active", "retry_wait", "verifying") }

    /** Recalcule tout : ce que le moteur peut faire, la protection, la notification. */
    fun refresh() {
        if (!app.downloads.loaded.value) return
        val busy = busy()
        val networkOk = connected && (!wifiOnly || wifi)
        val protectedNow = visible || job != null || service != null
        waiting.value = when {
            busy.isEmpty() -> null
            !connected -> R.string.wait_network
            !networkOk -> R.string.wait_wifi
            else -> null
        }
        val usable = networkOk && protectedNow
        // Réseau facturé au volume : le moteur ne partage pas les torrents (SPEC §7.16). Le
        // partage ne demande pas de protection : il n'a lieu que si l'application est à
        // l'écran ou qu'un téléchargement est protégé.
        val metered = !unmetered
        if (told != usable to metered) {
            told = usable to metered
            app.engine.call(
                "engine.setNetwork",
                JSONObject().put("usable", usable).put("metered", metered),
            )
        }
        if (busy.isEmpty()) {
            finishProtection()
        } else if (visible) {
            protect(busy)
        }
        if (bootPending && busy.isNotEmpty()) {
            bootPending = false
            offerResume()
        }
        updateNotice(busy)
    }

    // ---------- Protection ----------

    /** Demande la protection d'Android ; seulement l'application à l'écran (SPEC §7.15). */
    private fun protect(busy: List<Task>) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            if (job == null && scheduler().getPendingJob(JOB_ID) == null) scheduleJob(busy)
        } else if (service == null) {
            try {
                app.startForegroundService(Intent(app, TransferService::class.java))
            } catch (e: IllegalStateException) {
                Log.w(AuroApp.TAG, "service non démarré : $e")
            }
        }
    }

    private fun scheduler() = app.getSystemService(JobScheduler::class.java)

    /**
     * Le réseau par défaut passe par le Wi-Fi ou l'Ethernet. Un VPN a les transports de son
     * réseau sous-jacent à partir d'Android 9 ; avant, un VPN non facturé est pris pour du
     * Wi-Fi (le transport sous-jacent n'y est pas dit).
     */
    private fun isWifi(caps: NetworkCapabilities): Boolean =
        caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
            caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) ||
            (Build.VERSION.SDK_INT < Build.VERSION_CODES.P &&
                caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN) &&
                caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED))

    private fun scheduleJob(busy: List<Task>) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) return
        val left = busy.sumOf { t -> t.totalBytes?.let { (it - t.doneBytes).coerceAtLeast(0) } ?: 0L }
        val builder = JobInfo.Builder(JOB_ID, ComponentName(app, TransferJob::class.java))
            .setUserInitiated(true)
            .setEstimatedNetworkBytes(left.coerceAtLeast(1), 0)
        if (wifiOnly) {
            // Wi-Fi ou Ethernet, facturé ou non (audit C17) ; un VPN par-dessus est accepté
            // (la requête le refuse par défaut).
            builder.setRequiredNetwork(
                NetworkRequest.Builder()
                    .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
                    .removeCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
                    .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
                    .addTransportType(NetworkCapabilities.TRANSPORT_ETHERNET)
                    .build(),
            )
        } else {
            builder.setRequiredNetworkType(JobInfo.NETWORK_TYPE_ANY)
        }
        val info = builder.build()
        val result = scheduler().schedule(info)
        if (result != JobScheduler.RESULT_SUCCESS) Log.w(AuroApp.TAG, "transfert non programmé : $result")
    }

    /** « Wi-Fi seulement » changé : la contrainte du transfert suit. */
    private fun rescheduleJob() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) return
        val running = job
        if (running != null) {
            job = null
            running.first.jobFinished(running.second, false)
        }
        scheduler().cancel(JOB_ID)
    }

    /** Le transfert d'Android démarre (Android 14 et plus). */
    fun attach(service: JobService, params: JobParameters) {
        job = service to params
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            service.setNotification(params, NOTICE_ID, notice(busy()), JobService.JOB_END_NOTIFICATION_POLICY_REMOVE)
        }
        refresh()
    }

    /** Android arrête le transfert ; vrai s'il doit le reprendre plus tard. */
    fun detach(params: JobParameters): Boolean {
        if (job?.second == params) job = null
        refresh()
        return busy().isNotEmpty()
    }

    /** Le service au premier plan démarre (Android 8 à 13). */
    fun attach(service: TransferService) {
        this.service = service
        service.startInForeground(NOTICE_ID, notice(busy()))
        refresh()
    }

    fun detach(service: TransferService) {
        if (this.service === service) this.service = null
        refresh()
    }

    private fun finishProtection() {
        job?.let { (service, params) ->
            job = null
            service.jobFinished(params, false)
        }
        service?.let {
            service = null
            it.finish()
        }
    }

    // ---------- Notifications ----------

    private fun createChannels() {
        notifications.createNotificationChannel(
            NotificationChannel(CHANNEL_RUNNING, app.getString(R.string.channel_running), NotificationManager.IMPORTANCE_LOW),
        )
        notifications.createNotificationChannel(
            NotificationChannel(CHANNEL_DONE, app.getString(R.string.channel_done), NotificationManager.IMPORTANCE_DEFAULT),
        )
    }

    private fun openApp(): PendingIntent = PendingIntent.getActivity(
        app, 0, Intent(app, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    /** Notification des téléchargements en cours : nombre, avancement, débit ; « Tout mettre en pause ». */
    private fun notice(busy: List<Task>): Notification {
        val known = busy.filter { (it.totalBytes ?: 0) > 0 }
        val total = known.sumOf { it.totalBytes ?: 0 }
        val done = known.sumOf { minOf(it.doneBytes, it.totalBytes ?: 0) }
        val bps = busy.sumOf { it.bps }
        val wait = waiting.value
        val text = when {
            wait != null -> app.getString(wait)
            total > 0 -> app.getString(R.string.notice_progress, (done * 100 / total).toInt(), app.bytes(done), app.bytes(total)) +
                (if (bps > 0) " · " + app.rate(bps) else "")
            else -> app.getString(R.string.state_active)
        }
        val pauseAll = PendingIntent.getBroadcast(
            app, 1, Intent(app, ActionReceiver::class.java).setAction(ActionReceiver.PAUSE_ALL),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val builder = Notification.Builder(app, CHANNEL_RUNNING)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentTitle(app.resources.getQuantityString(R.plurals.notice_title, busy.size, busy.size))
            .setContentText(text)
            .setContentIntent(openApp())
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .addAction(Notification.Action.Builder(null, app.getString(R.string.pause_all), pauseAll).build())
        if (total > 0 && wait == null) {
            builder.setProgress(1000, (done * 1000 / total).toInt(), false)
        } else if (wait == null) {
            builder.setProgress(0, 0, true)
        }
        return builder.build()
    }

    /** Met la notification à jour, au plus une fois par seconde. */
    private fun updateNotice(busy: List<Task>) {
        if ((job == null && service == null) || busy.isEmpty()) return
        val now = SystemClock.elapsedRealtime()
        if (now - lastNotice < 1000) return
        lastNotice = now
        try {
            notifications.notify(NOTICE_ID, notice(busy))
        } catch (e: SecurityException) {
            // Notifications refusées : le transfert continue sans elles.
        }
    }

    /** Fin d'un téléchargement, l'application n'étant pas à l'écran. */
    fun notifyEnd(task: Task, failed: Boolean) {
        val notification = Notification.Builder(app, CHANNEL_DONE)
            .setSmallIcon(if (failed) android.R.drawable.stat_notify_error else android.R.drawable.stat_sys_download_done)
            .setContentTitle(app.getString(if (failed) R.string.notice_failed else R.string.notice_done))
            .setContentText(task.name)
            .setContentIntent(openApp())
            .setAutoCancel(true)
            .build()
        try {
            notifications.notify(task.id.hashCode(), notification)
        } catch (e: SecurityException) {
            // Notifications refusées.
        }
    }

    // ---------- Redémarrage de l'appareil ----------

    /**
     * Appareil redémarré : avec « Reprendre après le redémarrage », les téléchargements
     * inachevés repartent sous un service (Android 8 à 13) ; à partir d'Android 14, Android
     * ne laisse ni service ni transfert démarrer à ce moment, une notification propose de
     * les reprendre d'un geste.
     */
    fun onBoot() {
        if (!resumeAfterBoot) return
        bootPending = true
        refresh()
    }

    private fun offerResume() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            try {
                app.startForegroundService(Intent(app, TransferService::class.java))
                return
            } catch (e: IllegalStateException) {
                Log.w(AuroApp.TAG, "service non démarré au redémarrage : $e")
            }
        }
        val notification = Notification.Builder(app, CHANNEL_DONE)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentTitle(app.getString(R.string.notice_resume_title))
            .setContentText(app.getString(R.string.notice_resume_text))
            .setContentIntent(openApp())
            .setAutoCancel(true)
            .build()
        try {
            notifications.notify(RESUME_ID, notification)
        } catch (e: SecurityException) {
            // Notifications refusées.
        }
    }

    /** « Tout mettre en pause » : les tâches en cours ou en attente. */
    fun pauseAll() {
        val ids = JSONArray()
        for (task in app.downloads.tasks.values) if (task.canPause) ids.put(task.id)
        if (ids.length() > 0) app.engine.call("task.pause", JSONObject().put("taskIds", ids))
    }

    companion object {
        const val JOB_ID = 1
        const val NOTICE_ID = 1
        private const val RESUME_ID = 2
        private const val CHANNEL_RUNNING = "running"
        private const val CHANNEL_DONE = "done"
    }
}
