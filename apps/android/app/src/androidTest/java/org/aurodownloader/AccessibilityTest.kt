package org.aurodownloader

import android.Manifest
import android.graphics.Rect
import android.os.Build
import android.view.accessibility.AccessibilityNodeInfo
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.SemanticsNodeInteraction
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasScrollAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.isDialog
import androidx.compose.ui.test.junit4.accessibility.enableAccessibilityChecks
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.test.tryPerformAccessibilityChecks
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.rule.GrantPermissionRule
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.RuleChain
import org.junit.rules.TestRule
import org.junit.runner.RunWith

/**
 * Accessibilité de l'application (étape 7c, SPEC 9.1, WCAG 2.2 AA) : chaque écran passe les
 * contrôles de l'Accessibility Test Framework de Google (taille des cibles, contrastes,
 * noms lus par TalkBack…), à chaque action et à la demande. L'application tourne pour de
 * vrai (moteur compris) : à lancer sur un appareil dont les données sont à effacer ensuite.
 */
@RunWith(AndroidJUnit4::class)
class AccessibilityTest {
    // La permission des notifications ouvrirait une fenêtre du système par-dessus l'écran.
    @get:Rule(order = 0)
    val permissions: TestRule = if (Build.VERSION.SDK_INT >= 33) {
        GrantPermissionRule.grant(Manifest.permission.POST_NOTIFICATIONS)
    } else {
        RuleChain.emptyRuleChain()
    }

    @get:Rule(order = 1)
    val rule = createAndroidComposeRule<MainActivity>()

    private fun text(id: Int) = rule.activity.getString(id)

    private fun root(): SemanticsNodeInteraction = rule.onRoot()

    /**
     * Ce que lit TalkBack : l'arbre d'accessibilité d'Android, pas l'arbre sémantique de
     * Compose (essai TalkBack du 2026-11-18 : « Ajouter » sans nom, « Réglages » lu sur son
     * icône qui ne réagit pas, alors que Compose et l'ATF les voyaient nommés). Tout élément
     * cliquable porte son nom, ou le tient de textes à l'intérieur, jamais d'une icône seule.
     */
    private fun assertButtonsNamed(where: String) {
        rule.waitForIdle()
        val automation = InstrumentationRegistry.getInstrumentation().uiAutomation
        var root: AccessibilityNodeInfo? = null
        for (attempt in 0 until 20) {
            root = automation.rootInActiveWindow
            if (root != null) break
            Thread.sleep(100)
        }
        val faults = mutableListOf<String>()
        fun bounds(node: AccessibilityNodeInfo) = Rect().also { node.getBoundsInScreen(it) }
        // `view` : partie visible de la zone qui défile autour du nœud, s'il y en a une.
        fun walk(node: AccessibilityNodeInfo?, view: Rect?, visit: (AccessibilityNodeInfo, Rect?) -> Unit) {
            node ?: return
            visit(node, view)
            val inner = if (node.isScrollable) bounds(node) else view
            for (i in 0 until node.childCount) walk(node.getChild(i), inner, visit)
        }
        fun named(node: AccessibilityNodeInfo) = !node.text.isNullOrBlank() || !node.contentDescription.isNullOrBlank()
        walk(root ?: error("$where : aucune fenêtre active"), null) { node, view ->
            if (!node.isClickable || named(node)) return@walk
            val bounds = bounds(node)
            // Coupé par le bord de la zone qui défile : ses textes hors de l'écran ne sont pas
            // publiés par Android ; il est jugé dans une autre vue, une fois à l'écran.
            if (view != null && (bounds.top <= view.top || bounds.bottom >= view.bottom)) return@walk
            val inside = mutableListOf<AccessibilityNodeInfo>()
            for (i in 0 until node.childCount) walk(node.getChild(i), null) { child, _ -> inside += child }
            val texts = inside.filter { !it.text.isNullOrBlank() }
            val icons = inside.filter { !it.isClickable && it.text.isNullOrBlank() && !it.contentDescription.isNullOrBlank() }
            when {
                icons.isNotEmpty() -> faults += "$where : « ${icons.first().contentDescription} » est le nom d'une icône, pas de l'élément cliquable $bounds"
                texts.isEmpty() -> faults += "$where : élément cliquable sans nom $bounds"
            }
        }
        assertTrue(faults.joinToString("\n"), faults.isEmpty())
    }

    @Before
    fun checks() {
        rule.enableAccessibilityChecks()
    }

    @Test
    fun listeAjoutEtReglages() {
        root().tryPerformAccessibilityChecks()
        assertButtonsNamed("liste")

        // Le bouton tient son nom du texte qu'il contient (arbre fusionné, comme TalkBack).
        rule.onNode(hasText(text(R.string.add)) and hasClickAction()).performClick()
        rule.onNode(isDialog()).tryPerformAccessibilityChecks()
        assertButtonsNamed("boîte d'ajout")
        rule.onNodeWithText(text(R.string.cancel)).performClick()

        rule.onNodeWithContentDescription(text(R.string.settings)).performClick()
        // Tout l'écran, jusqu'aux torrents en bas : les contrôles ne voient que ce qui est à
        // l'écran, et une ligne coupée par un bord n'est jugée que dans une autre vue. Des pas
        // de 400 px, plus courts que la partie visible, montrent chaque ligne entière au moins
        // une fois.
        val scroller = rule.onNode(hasScrollAction())
        var view = 1
        while (true) {
            root().tryPerformAccessibilityChecks()
            assertButtonsNamed("réglages, vue $view")
            val range = scroller.fetchSemanticsNode().config[SemanticsProperties.VerticalScrollAxisRange]
            if (range.value() >= range.maxValue()) break
            assertTrue("réglages : défilement sans fin", view < 30)
            scroller.performSemanticsAction(SemanticsActions.ScrollBy) { it(0f, 400f) }
            rule.waitForIdle()
            view += 1
        }
        rule.onNodeWithContentDescription(text(R.string.back)).performClick()
        root().tryPerformAccessibilityChecks()
    }

    @Test
    fun ligneEtDetailsDUneTache() {
        val app = rule.activity.application as AuroApp
        // Rien n'écoute sur ce port : la tâche échoue vite, ou attend un réseau utilisable.
        val name = "essai-accessibilite.iso"
        rule.runOnUiThread { app.add("http://127.0.0.1:9/$name") }
        rule.waitUntil(20_000) {
            rule.onAllNodes(hasText(name, substring = true)).fetchSemanticsNodes().isNotEmpty()
        }
        root().tryPerformAccessibilityChecks()
        assertButtonsNamed("liste avec une tâche")

        rule.onNodeWithText(name).performClick()
        rule.waitForIdle()
        root().tryPerformAccessibilityChecks()
        assertButtonsNamed("détails")
        rule.onNodeWithContentDescription(text(R.string.back)).performClick()
    }
}
