package org.aurodownloader.ui

import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.path
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.aurodownloader.AuroApp
import org.aurodownloader.R
import org.aurodownloader.engine.Task
import org.aurodownloader.engine.TorrentFile
import org.json.JSONObject
import java.io.File

/** Icône « pause », absente du jeu d'icônes de base. */
private val PauseIcon: ImageVector = ImageVector.Builder("Pause", 24.dp, 24.dp, 24f, 24f).apply {
    path(fill = SolidColor(Color.Black)) {
        moveTo(6f, 5f); lineTo(10f, 5f); lineTo(10f, 19f); lineTo(6f, 19f); close()
        moveTo(14f, 5f); lineTo(18f, 5f); lineTo(18f, 19f); lineTo(14f, 19f); close()
    }
}.build()

/** Limites de débit proposées, en octets par seconde ; `null` : aucune. */
/** Partage des torrents (`torrent.ratioLimit`, SPEC §7.16) : ratios proposés, `null` sans limite. */
private const val RATIO_KEY = "torrent.ratioLimit"
private val RATIOS: List<Double?> = listOf(0.0, 0.5, 1.0, 2.0, 3.0, null)

/** Limites d'envoi proposées : plus basses que celles de la réception (connexion d'un
 * téléphone, envoi qui gêne la navigation). */
private val UPLOAD_LIMITS: List<Long?> = listOf(null, 100L shl 10, 256L shl 10, 512L shl 10, 1L shl 20, 2L shl 20, 5L shl 20)

private val LIMITS: List<Long?> =listOf(null, 256L shl 10, 1L shl 20, 2L shl 20, 5L shl 20, 10L shl 20, 20L shl 20, 50L shl 20)

@Composable
fun AuroTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val context = LocalContext.current
    val colors = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        dark -> darkColorScheme()
        else -> lightColorScheme()
    }
    // Certaines palettes dynamiques donnent au texte sur le fond une couleur bien plus
    // terne qu'aux surfaces (tablette Teclast, Android 15) : le même texte partout.
    MaterialTheme(colorScheme = colors.copy(onBackground = colors.onSurface), content = content)
}

/** Écrans de l'application : liste, détails d'un téléchargement, réglages ; ajout par une boîte. */
@Composable
fun AuroScreens(app: AuroApp, open: (Task) -> Unit) {
    if (!app.running) {
        Text(stringResource(R.string.engine_failed, app.failure ?: ""), Modifier.padding(24.dp))
        return
    }
    var screen by rememberSaveable { mutableStateOf("list") }
    var adding by rememberSaveable { mutableStateOf(false) }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val report: (Throwable?) -> Unit = { error ->
        if (error != null) {
            scope.launch { snackbar.showSnackbar(context.getString(R.string.refused, error.message ?: "")) }
        }
    }
    BackHandler(enabled = screen != "list") { screen = "list" }

    when {
        // Chaque écran montre les refus dans sa propre zone de messages (`snackbar`) : sans
        // elle, un refus dans les détails ou les réglages restait invisible (essai du
        // 2026-11-10).
        screen == "settings" -> SettingsScreen(app, snackbar, back = { screen = "list" }, report = report)
        screen.startsWith("details:") -> DetailsScreen(
            app, snackbar, screen.removePrefix("details:"), back = { screen = "list" }, open = open, report = report,
        )
        else -> ListScreen(
            app,
            snackbar = snackbar,
            details = { screen = "details:$it" },
            settings = { screen = "settings" },
            add = { adding = true },
            report = report,
        )
    }
    val added: (Throwable?) -> Unit = { error ->
        if (error == null) {
            scope.launch { snackbar.showSnackbar(context.getString(R.string.added)) }
        } else {
            report(error)
        }
    }
    // Fichier `.torrent` choisi par le sélecteur du système (étape 5e). Les fournisseurs ne
    // connaissent pas tous son type : tous les fichiers sont proposés, le contenu est vérifié.
    val pickTorrent = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) app.readTorrent(uri) { read -> read.fold({ app.addTorrent(it, added) }, report) }
    }
    if (adding) {
        AddDialog(
            dismiss = { adding = false },
            confirm = { url ->
                adding = false
                app.add(url, added)
            },
            chooseTorrent = {
                adding = false
                pickTorrent.launch(arrayOf("application/x-bittorrent", "application/octet-stream", "*/*"))
            },
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ListScreen(
    app: AuroApp,
    snackbar: SnackbarHostState,
    details: (String) -> Unit,
    settings: () -> Unit,
    add: () -> Unit,
    report: (Throwable?) -> Unit,
) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.app_name)) },
                actions = {
                    IconButton(onClick = settings) {
                        Icon(Icons.Filled.Settings, contentDescription = stringResource(R.string.settings))
                    }
                },
            )
        },
        floatingActionButton = {
            ExtendedFloatingActionButton(
                onClick = add,
                icon = { Icon(Icons.Filled.Add, contentDescription = null) },
                text = { Text(stringResource(R.string.add)) },
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        val tasks = app.downloads.list()
        val listState = rememberLazyListState()
        // Un téléchargement ajouté se place en tête : la liste y remonte si l'on regardait le
        // haut, sans quoi elle garderait en vue l'ancienne première ligne et le cacherait.
        val first = tasks.firstOrNull()?.id
        LaunchedEffect(first) {
            if (listState.firstVisibleItemIndex <= 1) listState.animateScrollToItem(0)
        }
        // Réseau qui ne convient pas : les téléchargements attendent (SPEC §7.15).
        val waiting = app.background.waiting.value
        Column(Modifier.padding(padding)) {
        if (waiting != null) {
            Text(
                stringResource(waiting),
                color = MaterialTheme.colorScheme.onSecondaryContainer,
                modifier = Modifier
                    .fillMaxWidth()
                    .background(MaterialTheme.colorScheme.secondaryContainer)
                    .padding(horizontal = 16.dp, vertical = 10.dp),
            )
        }
        if (app.downloads.loaded.value && tasks.isEmpty()) {
            Text(stringResource(R.string.empty), Modifier.padding(24.dp))
        } else {
            LazyColumn(state = listState, contentPadding = PaddingValues(bottom = 96.dp)) {
                items(tasks, key = { it.id }) { task ->
                    TaskRow(
                        task,
                        open = { details(task.id) },
                        toggle = when {
                            task.canPause -> { -> app.act("task.pause", task.id, done = report) }
                            task.canResume -> { -> app.act("task.resume", task.id, done = report) }
                            else -> null
                        },
                    )
                    HorizontalDivider()
                }
            }
        }
        }
    }
}

/** Une ligne de la liste : lue d'un seul tenant par TalkBack, avec son bouton à part. */
@Composable
private fun TaskRow(task: Task, open: () -> Unit, toggle: (() -> Unit)?) {
    val context = LocalContext.current
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(onClickLabel = stringResource(R.string.details), onClick = open)
            .padding(start = 16.dp, end = 4.dp, top = 10.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f).semantics(mergeDescendants = true) {}) {
            Text(
                task.name,
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.onSurface,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Spacer(Modifier.height(6.dp))
            Progress(task)
            Spacer(Modifier.height(4.dp))
            Text(
                context.summary(task),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        if (toggle != null) {
            val label = stringResource(
                when {
                    task.state == "seeding" -> R.string.stop_seeding
                    task.canPause -> R.string.pause
                    else -> R.string.resume
                },
            )
            IconButton(onClick = toggle, modifier = Modifier.semantics { contentDescription = "$label ${task.name}" }) {
                Icon(if (task.canPause) PauseIcon else Icons.Filled.PlayArrow, contentDescription = null)
            }
        }
    }
}

@Composable
private fun Progress(task: Task) {
    val color = when (task.state) {
        "completed", "seeding" -> Color(0xFF43A047)
        "failed", "cancelled" -> MaterialTheme.colorScheme.error
        "active", "verifying" -> MaterialTheme.colorScheme.primary
        else -> MaterialTheme.colorScheme.outline
    }
    val fraction = task.fraction
    val modifier = Modifier.fillMaxWidth().height(6.dp)
    when {
        fraction != null -> LinearProgressIndicator(progress = { fraction }, modifier = modifier, color = color)
        task.state == "active" -> LinearProgressIndicator(modifier = modifier, color = color)
        else -> LinearProgressIndicator(progress = { 0f }, modifier = modifier, color = color)
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun DetailsScreen(
    app: AuroApp,
    snackbar: SnackbarHostState,
    id: String,
    back: () -> Unit,
    open: (Task) -> Unit,
    report: (Throwable?) -> Unit,
) {
    val task = app.downloads.tasks[id]
    if (task == null) {
        // Retirée entre-temps.
        LaunchedEffect(id) { back() }
        return
    }
    val context = LocalContext.current
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.details)) },
                navigationIcon = {
                    IconButton(onClick = back) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = stringResource(R.string.back))
                    }
                },
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        Column(
            Modifier.padding(padding).verticalScroll(rememberScrollState()).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(task.name, style = MaterialTheme.typography.titleLarge)
            Progress(task)
            Fact(R.string.f_state, context.stateText(task))
            task.fileName?.let { Fact(R.string.f_file, it) }
            Fact(R.string.f_folder, task.destDir)
            Fact(R.string.f_size, task.totalBytes?.let { context.bytes(it) } ?: stringResource(R.string.unknown_size))
            Fact(R.string.f_done, context.doneText(task))
            if (task.state == "active") {
                Fact(R.string.f_rate, context.rate(task.bps))
                context.duration(task.etaS).takeIf { it.isNotEmpty() }?.let { Fact(R.string.f_eta, it) }
            }
            // Torrent : ce qui est envoyé aux autres pairs, et le ratio (SPEC §7.16).
            task.uploadedBytes?.takeIf { task.isTorrent }?.let { sent ->
                val text = task.shareRatio?.let { stringResource(R.string.uploaded_ratio, context.bytes(sent), context.ratio(it)) }
                    ?: context.bytes(sent)
                val rate = if (task.uploadBps > 0) " · " + stringResource(R.string.upload_rate, context.rate(task.uploadBps)) else ""
                Fact(R.string.f_uploaded, text + rate)
            }
            task.checksum?.let { sum ->
                val algo = when (sum.optString("algo")) {
                    "sha256" -> "SHA-256"
                    "sha1" -> "SHA-1"
                    "md5" -> "MD5"
                    else -> sum.optString("algo")
                }
                val value = sum.optString("value")
                val source = if (sum.isNull("source")) null else sum.optString("source")
                var text = if (source != null) {
                    stringResource(R.string.checksum_from, algo, value, source)
                } else {
                    stringResource(R.string.checksum_value, algo, value)
                }
                if (task.state == "completed") text = stringResource(R.string.checksum_verified, text)
                Fact(R.string.f_checksum, text)
            }
            Fact(R.string.f_added, context.date(task.createdAt))
            task.errorDetail?.takeIf { task.state == "failed" }?.let {
                Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
            }
            // SFTP (SPEC §7.22) : la clé du serveur attend la décision de l'utilisateur.
            if (task.state == "failed" && task.errorCode in setOf("HOST_KEY_UNKNOWN", "HOST_KEY_CHANGED")) {
                HostKey(app, task, report)
            }
            if (task.isTorrent) TorrentFiles(app, task, report)
            Spacer(Modifier.height(8.dp))
            val wide = Modifier.fillMaxWidth()
            if (task.canPause) {
                // Arrêter un torrent qui partage le termine ; ses fichiers restent.
                val label = if (task.state == "seeding") R.string.stop_seeding else R.string.pause
                Button(onClick = { app.act("task.pause", id, done = report) }, modifier = wide) { Text(stringResource(label)) }
            }
            if (task.canResume) {
                val label = if (task.state == "failed") R.string.retry else R.string.resume
                Button(onClick = { app.act("task.resume", id, done = report) }, modifier = wide) { Text(stringResource(label)) }
            }
            // Un fichier seulement : un torrent à plusieurs fichiers est un dossier.
            val name = task.fileName
            val single = remember(task.destDir, name, task.state) { name != null && File(task.destDir, name).isFile }
            if (task.isFinished && single) {
                Button(onClick = { open(task) }, modifier = wide) { Text(stringResource(R.string.open)) }
            }
            // Annuler abandonne le téléchargement : son `.part` part avec lui, comme sur le
            // bureau. Un torrent qui partage s'arrête par « Arrêter le partage ».
            if (task.canCancel && task.state != "seeding") {
                OutlinedButton(
                    onClick = { app.act("task.cancel", id, JSONObject().put("deleteFiles", true), done = report) },
                    modifier = wide,
                ) {
                    Text(stringResource(R.string.cancel_download))
                }
            }
            if (task.canRestart) {
                OutlinedButton(onClick = { app.act("task.restart", id, done = report) }, modifier = wide) {
                    Text(stringResource(R.string.restart))
                }
            }
            // Retirer de la liste : un fichier terminé reste ; le `.part` d'une tâche en échec
            // ou annulée part (sinon il resterait sans tâche, SPEC §6.5), comme sur le bureau.
            if (task.canRemove) {
                val extra = JSONObject().put("deleteFiles", task.state != "completed")
                OutlinedButton(onClick = { app.act("task.remove", id, extra, done = report) }, modifier = wide) {
                    Text(stringResource(R.string.remove))
                }
            }
        }
    }
}

/**
 * Clé du serveur SSH d'une tâche arrêtée par elle (SFTP, SPEC §7.22) : son empreinte, et
 * « Accepter la clé… » après confirmation ; le moteur relance alors la tâche. Après un
 * redémarrage du moteur, la clé n'est plus en attente : relancer la tâche la remontre.
 */
@Composable
private fun HostKey(app: AuroApp, task: Task, report: (Throwable?) -> Unit) {
    var key by remember(task.id) { mutableStateOf<AuroApp.PendingKey?>(null) }
    var asking by remember(task.id) { mutableStateOf(false) }
    LaunchedEffect(task.id, task.state, task.errorCode) { app.pendingKey(task.id) { key = it } }
    val found = key
    if (found == null) {
        Text(stringResource(R.string.host_key_gone), style = MaterialTheme.typography.bodySmall)
        return
    }
    val fingerprint = "${found.algorithm} ${found.fingerprint}"
    Text(
        stringResource(if (found.changed) R.string.host_key_changed_note else R.string.host_key_unknown_note),
        style = MaterialTheme.typography.bodySmall,
        color = if (found.changed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
    )
    Text(fingerprint, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace)
    Button(onClick = { asking = true }, modifier = Modifier.fillMaxWidth()) {
        Text(stringResource(R.string.host_key_accept))
    }
    if (asking) {
        AlertDialog(
            onDismissRequest = { asking = false },
            title = { Text(stringResource(R.string.host_key_confirm_title)) },
            text = {
                Text(stringResource(if (found.changed) R.string.host_key_confirm_changed else R.string.host_key_confirm, fingerprint))
            },
            confirmButton = {
                TextButton(onClick = {
                    asking = false
                    app.acceptKey(found, report)
                }) { Text(stringResource(R.string.host_key_accept_short)) }
            },
            dismissButton = { TextButton(onClick = { asking = false }) { Text(stringResource(R.string.cancel)) } },
        )
    }
}

/**
 * Fichiers d'un torrent (`task.get` avec `files`) : relus quand sa taille ou son état change
 * (métadonnées reçues, choix appliqué). Le choix se modifie tant que le torrent n'est ni reçu
 * ni annulé (`task.setFiles`, au moins un fichier), comme sur le bureau.
 */
@Composable
private fun TorrentFiles(app: AuroApp, task: Task, report: (Throwable?) -> Unit) {
    val context = LocalContext.current
    var files by remember(task.id) { mutableStateOf<List<TorrentFile>?>(null) }
    var chosen by remember(task.id) { mutableStateOf<Set<Int>>(emptySet()) }
    // Choix en cours de l'utilisateur, pas encore appliqué : une relecture ne l'efface pas.
    var dirty by remember(task.id) { mutableStateOf(false) }
    LaunchedEffect(task.id, task.totalBytes, task.state) {
        app.files(task.id) { reply ->
            reply.onSuccess { list ->
                files = list
                if (!dirty) chosen = list.filter { it.selected }.map { it.index }.toSet()
            }
        }
    }
    Spacer(Modifier.height(8.dp))
    Text(stringResource(R.string.torrent_files), style = MaterialTheme.typography.titleSmall)
    val list = files
    if (list.isNullOrEmpty()) {
        Text(stringResource(R.string.torrent_files_wait), style = MaterialTheme.typography.bodySmall)
        return
    }
    val editable = task.canChooseFiles
    val size = list.filter { it.index in chosen }.sumOf { it.length }
    Text(
        stringResource(R.string.torrent_files_summary, chosen.size, context.bytes(size)),
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    for (file in list) {
        val on = file.index in chosen
        Row(
            Modifier
                .fillMaxWidth()
                .toggleable(value = on, enabled = editable, role = Role.Checkbox) {
                    chosen = if (it) chosen + file.index else chosen - file.index
                    dirty = true
                }
                .padding(vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Checkbox(checked = on, onCheckedChange = null, enabled = editable)
            Spacer(Modifier.width(8.dp))
            Text(stringResource(R.string.torrent_file_row, file.path, context.bytes(file.length)))
        }
    }
    if (editable && dirty) {
        Button(
            onClick = {
                if (chosen.isEmpty()) {
                    report(Exception(context.getString(R.string.torrent_files_none)))
                } else {
                    app.setFiles(task.id, chosen.sorted()) { error ->
                        report(error)
                        if (error == null) dirty = false
                    }
                }
            },
            modifier = Modifier.fillMaxWidth(),
        ) { Text(stringResource(R.string.torrent_files_apply)) }
    }
}

/** Une ligne des détails : libellé et valeur, lus ensemble. */
@Composable
private fun Fact(label: Int, value: String) {
    Column(Modifier.fillMaxWidth().semantics(mergeDescendants = true) {}) {
        Text(stringResource(label), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value, style = MaterialTheme.typography.bodyMedium)
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SettingsScreen(app: AuroApp, snackbar: SnackbarHostState, back: () -> Unit, report: (Throwable?) -> Unit) {
    val context = LocalContext.current
    val settings = app.downloads.settings
    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.settings)) },
                navigationIcon = {
                    IconButton(onClick = back) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = stringResource(R.string.back))
                    }
                },
            )
        },
    ) { padding ->
        Column(Modifier.padding(padding).verticalScroll(rememberScrollState()).padding(16.dp)) {
            Fact(R.string.folder, settings["downloads.dir"]?.toString() ?: "")
            Spacer(Modifier.height(16.dp))
            Text(stringResource(R.string.limit), style = MaterialTheme.typography.titleSmall)
            Column(Modifier.selectableGroup()) {
                val current = app.downloads.downloadLimit.value
                for (limit in LIMITS) {
                    val label = limit?.let { context.rate(it) } ?: stringResource(R.string.no_limit)
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .selectable(selected = current == limit, role = Role.RadioButton) { app.setLimit(limit, report) }
                            .padding(vertical = 6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        RadioButton(selected = current == limit, onClick = null)
                        Spacer(Modifier.width(12.dp))
                        Text(label)
                    }
                }
            }
            Spacer(Modifier.height(16.dp))
            Toggle(
                label = stringResource(R.string.neighbor),
                note = stringResource(R.string.neighbor_note),
                on = settings["downloads.neighborChecksum"] != false,
                change = { app.setSetting("downloads.neighborChecksum", it, report) },
            )
            Toggle(
                label = stringResource(R.string.preallocate),
                note = stringResource(R.string.preallocate_note),
                on = settings["downloads.preallocate"] != false,
                change = { app.setSetting("downloads.preallocate", it, report) },
            )
            // Préférences de l'application Android (arrière-plan, SPEC §7.15).
            var wifiOnly by remember { mutableStateOf(app.background.wifiOnly) }
            Toggle(
                label = stringResource(R.string.wifi_only),
                note = stringResource(R.string.wifi_only_note),
                on = wifiOnly,
                change = {
                    app.background.wifiOnly = it
                    wifiOnly = it
                },
            )
            var afterBoot by remember { mutableStateOf(app.background.resumeAfterBoot) }
            Toggle(
                label = stringResource(R.string.resume_boot),
                note = stringResource(R.string.resume_boot_note),
                on = afterBoot,
                change = {
                    app.background.resumeAfterBoot = it
                    afterBoot = it
                },
            )
            // Torrents (étape 5e, comme la rubrique BitTorrent du bureau). Pas d'interface
            // réseau sous Android : le VPN d'Android vaut pour toute l'application (décision
            // du 2026-11-10).
            Spacer(Modifier.height(16.dp))
            Text(stringResource(R.string.bittorrent), style = MaterialTheme.typography.titleMedium)
            Spacer(Modifier.height(8.dp))
            Text(stringResource(R.string.seed_after), style = MaterialTheme.typography.titleSmall)
            val ratio = if (settings.containsKey(RATIO_KEY)) (settings[RATIO_KEY] as? Number)?.toDouble() else 1.0
            val ratios = if (ratio in RATIOS) RATIOS else (RATIOS + ratio).sortedBy { it ?: Double.MAX_VALUE }
            Column(Modifier.selectableGroup()) {
                for (value in ratios) {
                    val label = when (value) {
                        null -> stringResource(R.string.seed_unlimited)
                        0.0 -> stringResource(R.string.seed_none)
                        1.0 -> stringResource(R.string.seed_one)
                        else -> stringResource(R.string.seed_upto, context.ratioChoice(value))
                    }
                    Choice(label, selected = ratio == value) { app.setSetting(RATIO_KEY, value, report) }
                }
            }
            Text(
                stringResource(R.string.seed_note),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(16.dp))
            Text(stringResource(R.string.upload_limit), style = MaterialTheme.typography.titleSmall)
            Column(Modifier.selectableGroup()) {
                val current = app.downloads.uploadLimit.value
                for (limit in UPLOAD_LIMITS) {
                    val label = limit?.let { context.rate(it) } ?: stringResource(R.string.no_limit)
                    Choice(label, selected = current == limit) { app.setUploadLimit(limit, report) }
                }
            }
        }
    }
}

/** Un choix d'une liste à bouton radio. */
@Composable
private fun Choice(label: String, selected: Boolean, choose: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .selectable(selected = selected, role = Role.RadioButton, onClick = choose)
            .padding(vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RadioButton(selected = selected, onClick = null)
        Spacer(Modifier.width(12.dp))
        Text(label)
    }
}

@Composable
private fun Toggle(label: String, note: String, on: Boolean, change: (Boolean) -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .toggleable(value = on, role = Role.Switch, onValueChange = change)
            .padding(vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(label)
            Text(note, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Spacer(Modifier.width(12.dp))
        Switch(checked = on, onCheckedChange = null)
    }
}

/** Boîte « Ajouter » : une adresse http(s), ftp(s) ou sftp, un lien magnet, ou un fichier `.torrent` choisi. */
@Composable
private fun AddDialog(dismiss: () -> Unit, confirm: (String) -> Unit, chooseTorrent: () -> Unit) {
    var url by rememberSaveable { mutableStateOf("") }
    var wrong by rememberSaveable { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = dismiss,
        title = { Text(stringResource(R.string.add_title)) },
        text = {
            Column {
                OutlinedTextField(
                    value = url,
                    onValueChange = {
                        url = it
                        wrong = false
                    },
                    label = { Text(stringResource(R.string.address)) },
                    placeholder = { Text(stringResource(R.string.address_hint)) },
                    singleLine = true,
                    isError = wrong,
                    supportingText = if (wrong) {
                        { Text(stringResource(R.string.address_invalid)) }
                    } else {
                        null
                    },
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(8.dp))
                OutlinedButton(onClick = chooseTorrent, modifier = Modifier.fillMaxWidth()) {
                    Text(stringResource(R.string.choose_torrent))
                }
            }
        },
        confirmButton = {
            TextButton(onClick = {
                val text = url.trim()
                val accepted = listOf("http://", "https://", "ftp://", "ftps://", "sftp://", "magnet:?")
                    .any { text.startsWith(it, ignoreCase = true) }
                if (accepted) confirm(text) else wrong = true
            }) { Text(stringResource(R.string.add)) }
        },
        dismissButton = { TextButton(onClick = dismiss) { Text(stringResource(R.string.cancel)) } },
    )
}
