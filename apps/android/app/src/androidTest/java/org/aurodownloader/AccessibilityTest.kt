package org.aurodownloader

import android.Manifest
import android.os.Build
import androidx.compose.ui.test.SemanticsNodeInteraction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.isDialog
import androidx.compose.ui.test.junit4.accessibility.enableAccessibilityChecks
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.tryPerformAccessibilityChecks
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.rule.GrantPermissionRule
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

    @Before
    fun checks() {
        rule.enableAccessibilityChecks()
    }

    @Test
    fun listeAjoutEtReglages() {
        root().tryPerformAccessibilityChecks()

        rule.onNodeWithText(text(R.string.add), useUnmergedTree = true).performClick()
        rule.onNode(isDialog()).tryPerformAccessibilityChecks()
        rule.onNodeWithText(text(R.string.cancel)).performClick()

        rule.onNodeWithContentDescription(text(R.string.settings)).performClick()
        root().tryPerformAccessibilityChecks()
        // Le bas des réglages (torrents) : les contrôles ne voient que ce qui est à l'écran.
        rule.onNodeWithText(text(R.string.upload_limit)).performScrollTo()
        root().tryPerformAccessibilityChecks()
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

        rule.onNodeWithText(name).performClick()
        rule.waitForIdle()
        root().tryPerformAccessibilityChecks()
        rule.onNodeWithContentDescription(text(R.string.back)).performClick()
    }
}
