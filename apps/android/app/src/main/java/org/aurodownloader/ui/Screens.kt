package org.aurodownloader.ui

import android.os.Build
import androidx.activity.compose.BackHandler
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
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.aurodownloader.AuroApp
import org.aurodownloader.R
import org.aurodownloader.engine.Task

/** Icône « pause », absente du jeu d'icônes de base. */
private val PauseIcon: ImageVector = ImageVector.Builder("Pause", 24.dp, 24.dp, 24f, 24f).apply {
    path(fill = SolidColor(Color.Black)) {
        moveTo(6f, 5f); lineTo(10f, 5f); lineTo(10f, 19f); lineTo(6f, 19f); close()
        moveTo(14f, 5f); lineTo(18f, 5f); lineTo(18f, 19f); lineTo(14f, 19f); close()
    }
}.build()

/** Limites de débit proposées, en octets par seconde ; `null` : aucune. */
private val LIMITS: List<Long?> = listOf(null, 256L shl 10, 1L shl 20, 2L shl 20, 5L shl 20, 10L shl 20, 20L shl 20, 50L shl 20)

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
        screen == "settings" -> SettingsScreen(app, back = { screen = "list" }, report = report)
        screen.startsWith("details:") -> DetailsScreen(
            app, screen.removePrefix("details:"), back = { screen = "list" }, open = open, report = report,
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
    if (adding) {
        AddDialog(
            dismiss = { adding = false },
            confirm = { url ->
                adding = false
                app.add(url) { error ->
                    if (error == null) {
                        scope.launch { snackbar.showSnackbar(context.getString(R.string.added)) }
                    } else {
                        report(error)
                    }
                }
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
            val label = stringResource(if (task.canPause) R.string.pause else R.string.resume)
            IconButton(onClick = toggle, modifier = Modifier.semantics { contentDescription = "$label ${task.name}" }) {
                Icon(if (task.canPause) PauseIcon else Icons.Filled.PlayArrow, contentDescription = null)
            }
        }
    }
}

@Composable
private fun Progress(task: Task) {
    val color = when (task.state) {
        "completed" -> Color(0xFF43A047)
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
private fun DetailsScreen(app: AuroApp, id: String, back: () -> Unit, open: (Task) -> Unit, report: (Throwable?) -> Unit) {
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
            Spacer(Modifier.height(8.dp))
            val wide = Modifier.fillMaxWidth()
            if (task.canPause) {
                Button(onClick = { app.act("task.pause", id, done = report) }, modifier = wide) { Text(stringResource(R.string.pause)) }
            }
            if (task.canResume) {
                val label = if (task.state == "failed") R.string.retry else R.string.resume
                Button(onClick = { app.act("task.resume", id, done = report) }, modifier = wide) { Text(stringResource(label)) }
            }
            if (task.state == "completed" && task.fileName != null) {
                Button(onClick = { open(task) }, modifier = wide) { Text(stringResource(R.string.open)) }
            }
            if (task.canCancel) {
                OutlinedButton(onClick = { app.act("task.cancel", id, done = report) }, modifier = wide) {
                    Text(stringResource(R.string.cancel_download))
                }
            }
            if (task.canRestart) {
                OutlinedButton(onClick = { app.act("task.restart", id, done = report) }, modifier = wide) {
                    Text(stringResource(R.string.restart))
                }
            }
            if (task.canRemove) {
                OutlinedButton(onClick = { app.act("task.remove", id, done = report) }, modifier = wide) {
                    Text(stringResource(R.string.remove))
                }
            }
        }
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
private fun SettingsScreen(app: AuroApp, back: () -> Unit, report: (Throwable?) -> Unit) {
    val context = LocalContext.current
    val settings = app.downloads.settings
    Scaffold(
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
        }
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

@Composable
private fun AddDialog(dismiss: () -> Unit, confirm: (String) -> Unit) {
    var url by rememberSaveable { mutableStateOf("") }
    var wrong by rememberSaveable { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = dismiss,
        title = { Text(stringResource(R.string.add_title)) },
        text = {
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
        },
        confirmButton = {
            TextButton(onClick = {
                val text = url.trim()
                if (text.startsWith("http://", ignoreCase = true) || text.startsWith("https://", ignoreCase = true)) {
                    confirm(text)
                } else {
                    wrong = true
                }
            }) { Text(stringResource(R.string.add)) }
        },
        dismissButton = { TextButton(onClick = dismiss) { Text(stringResource(R.string.cancel)) } },
    )
}
