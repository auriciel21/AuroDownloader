package org.aurodownloader.ui

import android.content.Context
import org.aurodownloader.R
import org.aurodownloader.engine.Task
import java.text.DateFormat
import java.util.Date
import java.util.Locale

/** Taille en unités binaires, une décimale au-delà de l'octet (comme sur le bureau). */
fun Context.bytes(n: Long?): String {
    if (n == null || n < 0) return "?"
    val units = intArrayOf(R.string.unit_b, R.string.unit_kib, R.string.unit_mib, R.string.unit_gib, R.string.unit_tib)
    var value = n.toDouble()
    var i = 0
    while (value >= 1024 && i < units.size - 1) {
        value /= 1024
        i++
    }
    val locale = resources.configuration.locales[0] ?: Locale.getDefault()
    val number = if (i == 0) n.toString() else String.format(locale, "%.1f", value)
    return "$number ${getString(units[i])}"
}

fun Context.rate(bps: Long): String = getString(R.string.per_second, bytes(bps))

/** Durée arrondie à l'unité utile ; vide si inconnue. */
fun Context.duration(seconds: Long?): String {
    val s = seconds ?: return ""
    if (s < 0) return ""
    return when {
        s < 60 -> getString(R.string.eta_s, s.toInt())
        s < 3600 -> getString(R.string.eta_min, (s / 60).toInt(), (s % 60).toInt())
        s < 86400 -> getString(R.string.eta_h, (s / 3600).toInt(), ((s % 3600) / 60).toInt())
        else -> getString(R.string.eta_d, (s / 86400).toInt(), ((s % 86400) / 3600).toInt())
    }
}

fun Context.date(ms: Long): String =
    DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT).format(Date(ms))

/** État lisible, avec la cause d'une pause qui ne vient pas de l'utilisateur ou d'un échec. */
fun Context.stateText(task: Task): String {
    val verified = task.verifiedBytes
    val total = task.totalBytes
    if (task.state == "verifying" && verified != null && total != null && total > 0) {
        return getString(R.string.state_verifying_pct, (minOf(verified, total) * 100 / total).toInt())
    }
    val state = when (task.state) {
        "queued" -> getString(R.string.state_queued)
        "active" -> getString(R.string.state_active)
        "paused" -> getString(R.string.state_paused)
        "retry_wait" -> getString(R.string.state_retry_wait)
        "verifying" -> getString(R.string.state_verifying)
        "completed" -> getString(R.string.state_completed)
        "failed" -> getString(R.string.state_failed)
        "cancelled" -> getString(R.string.state_cancelled)
        else -> task.state
    }
    val reason = task.pauseReason
    if (task.state == "paused" && reason != null && reason != "user") {
        return getString(R.string.state_with_reason, state, named("reason_$reason") ?: reason)
    }
    val code = task.errorCode
    if ((task.state == "failed" || task.state == "retry_wait") && code != null) {
        return getString(R.string.state_with_error, state, named("error_$code") ?: code)
    }
    return state
}

/** Octets reçus, sur la taille si elle est connue. */
fun Context.doneText(task: Task): String {
    val total = task.totalBytes ?: return bytes(task.doneBytes)
    return getString(R.string.done_of, bytes(task.doneBytes), bytes(total))
}

/** Ligne d'état d'une tâche dans la liste : état, octets, débit et temps restant. */
fun Context.summary(task: Task): String {
    val parts = mutableListOf(stateText(task), doneText(task))
    if (task.state == "active") {
        if (task.bps > 0) parts += rate(task.bps)
        duration(task.etaS).takeIf { it.isNotEmpty() }?.let { parts += getString(R.string.eta_left, it) }
    }
    return parts.joinToString(" · ")
}

private fun Context.named(name: String): String? {
    val id = resources.getIdentifier(name, "string", packageName)
    return if (id == 0) null else getString(id)
}
