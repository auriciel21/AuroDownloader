# Revue d'accessibilité : WCAG 2.2, niveaux A et AA

Phase 7, étape 7 (exigence N-09, SPEC 3 et 9.1), revue du 2026-11-18. Cible : la conformité
WCAG 2.2 niveau AA de l'interface graphique.

**Méthode**, décidée par l'utilisateur le 2026-11-18 :
- des tests automatiques sur chaque interface ;
- la lecture du code et des styles, critère par critère ;
- des essais réels avec un lecteur d'écran sur chaque système, faits par l'utilisateur ;
- les défauts corrigés au fil, chacun couvert par un test quand c'est possible.

## 1. Périmètre

| Interface | Code | Lecteur d'écran de l'essai |
|---|---|---|
| Application de bureau (Windows, Linux) | page `ui/src` dans Tauri (WebView2, WebKitGTK) | NVDA (Windows), Orca (Linux) |
| Interface web distante | la même page, servie par le moteur | celui du navigateur ou du téléphone |
| Application Android | `apps/android` (Jetpack Compose, Material 3) | TalkBack |
| Extension (fenêtre de l'icône, options) | `extension/popup.*`, `extension/options.*` | NVDA, dans le navigateur |

Hors périmètre :
- la CLI : texte en terminal, lu tel quel par les lecteurs d'écran ;
- les menus contextuels et les notifications du navigateur et du système : dessinés par
  eux.

## 2. Tests automatiques

| Test | Ce qu'il vérifie | Ses limites |
|---|---|---|
| `ui/tests/axe.test.js` | axe-core 4.10 (règles WCAG 2.0 à 2.2, A et AA, et bonnes pratiques) sur le balisage de la fenêtre et de chacune des 20 boîtes. Le contenu caché est montré : messages d'erreur, sections selon l'état | Sans mise en page : ni contraste, ni taille, ni focus |
| `ui/tests/live.test.js` | La vraie page (`main.js`), branchée à un faux moteur qui tient des tâches dans tous les états. axe-core passe sur ce que le script dessine : lignes, arborescence, Propriétés de chaque état, clé SSH, menu, chaque rubrique des Options, boîtes de la barre d'outils. Le parcours au clavier est vérifié : flèches, Entrée, Maj+F10, Échap, focus qui entre dans une boîte puis revient. Les messages pendant qu'une boîte est ouverte sont vérifiés aussi (A14 à A17) : zones de messages de chaque boîte présentes dès le chargement, erreur dans la boîte et liée au champ avant qu'il prenne le focus, boîte « Ajouter » ouverte jusqu'au succès | jsdom ne montre rien : le focus de `showModal` et de `close` est imité, selon la norme HTML ; ce que lit vraiment un lecteur d'écran (ordre, coupures) ne se voit qu'aux essais réels |
| `ui/tests/contrast.test.js` | Les contrastes des couleurs de `style.css`, thèmes clair et sombre, couleur par couleur selon leur usage réel : 4,5:1 pour le texte (1.4.3) ; 3:1 pour le focus, les icônes et les bords des champs (1.4.11) | Couleurs pleines seulement ; les dégradés des barres de progression sont vus à la main |
| `extension/tests/a11y.test.cjs` | axe-core sur la fenêtre de l'icône (vidéo décrite, relais absent) et sur les options, avec leurs scripts, en français et en anglais ; contrastes d'`options.css` | Comme la page |
| `apps/android/app/src/androidTest/…/AccessibilityTest.kt` | L'application réelle, avec les contrôles de l'Accessibility Test Framework de Google à chaque action : liste vide et avec une tâche, boîte d'ajout, réglages vue par vue jusqu'en bas, détails. Ces contrôles couvrent la taille des cibles, les noms lus, les doublons et les contrastes que l'outil sait mesurer. En plus, l'arbre d'accessibilité réel d'Android, celui que lit TalkBack : tout élément cliquable porte son nom ou le tient de textes à l'intérieur, jamais d'une icône seule (A12, A13) | La section de la clé SSH demande un vrai serveur SFTP : elle est vue à l'essai réel (capture du 2026-11-17, essai TalkBack du 2026-11-18). Un élément coupé par le bord d'une zone qui défile n'est jugé que dans une vue où il est entier |

**Garde-fous des tests.**
- **axe.** Une règle qu'axe n'a pas pu exécuter compte comme un échec. Ce n'est pas
  théorique : une boîte ouverte lui faisait sauter toutes ses règles faute
  d'`elementFromPoint` dans jsdom, et les 20 boîtes « passaient » sans rien vérifier.
- **Défaut glissé.** Les tests de la page et de l'extension glissent un défaut (bouton
  sans nom, champ sans étiquette) et exigent qu'il soit vu.
- **Défauts remis.** Le contrôle de l'arbre réel d'Android a été essayé contre les deux
  défauts trouvés par TalkBack, remis un à un dans l'application : il échoue sur chacun
  (2026-11-18). De même, les tests des messages de la page échouent sur l'ancien code et
  sur la version intermédiaire essayée avec NVDA (erreur écrite après la réouverture de la
  boîte).

**Lancement.**
- Page et extension : `cd ui && npm ci --ignore-scripts`, puis `node --test ui/tests/
  extension/tests/`. axe-core et jsdom sont des dépendances de développement, jamais
  livrées (SPEC 8.3).
- Android : sur un appareil, par `am instrument` (`CLAUDE.md`).
- Les trois tourneront dans l'intégration continue de l'étape 8 ; Android y tournera sur
  l'émulateur.

## 3. Revue critère par critère

Légende des statuts :

| Statut | Sens |
|---|---|
| Oui | Conforme. Le test ou la vérification est indiqué |
| Corrigé | Défaut trouvé par cette revue, corrigé (section 4) |
| S. o. | Sans objet |
| Essai | À confirmer par l'essai réel (section 6) |

Colonnes :
- **Page** : l'application de bureau et l'interface distante.
- **Android** et **Ext.** (extension) : leur statut est le même que celui de la page quand
  la case est vide.

| Critère | Niv. | Page | Android | Ext. | Vérification, remarques |
|---|---|---|---|---|---|
| 1.1.1 Contenu non textuel | A | Corrigé | Oui | Oui | Les icônes ont `aria-hidden` à côté d'un texte. La carte des segments (`role="img"`) n'avait pas de nom. Android : icônes avec `contentDescription`, ou décoratives à `null` à côté d'un texte (ATF) |
| 1.2.1 à 1.2.5 Médias temporels | A, AA | S. o. | S. o. | S. o. | Aucune vidéo ni aucun son dans l'interface ; les vidéos téléchargées ne sont pas lues par l'application |
| 1.3.1 Information et relations | A | Corrigé | Oui | Oui | Repères `header`, `nav`, `main`, `footer` (la barre d'outils était hors de tout repère). Tableau `role="grid"` avec `aria-sort` et `aria-selected`. Groupes de champs avec `fieldset` et `legend`. Boîtes nommées par leur titre. Android : `selectableGroup` et `Role.RadioButton` pour les choix |
| 1.3.2 Ordre séquentiel logique | A | Oui | Oui | Oui | L'ordre du DOM suit l'ordre visuel ; aucun `tabindex` positif |
| 1.3.3 Caractéristiques sensorielles | A | Oui | Oui | Oui | Aucune consigne ne repose sur la forme, la place ou la couleur seules |
| 1.3.4 Orientation | AA | Oui | Oui | Oui | Aucune orientation imposée : manifeste Android sans `screenOrientation`, page fluide |
| 1.3.5 Finalité des champs | AA | Oui | S. o. | S. o. | Aucun champ ne recueille des données de l'utilisateur au sens du critère : les identifiants de sites et du proxy sont ceux de tiers, rangés au coffre, et leurs champs gardent `autocomplete="off"` (choix discutable, signalé à l'utilisateur). Le jeton de l'interface distante a `autocomplete="new-password"` |
| 1.4.1 Utilisation de la couleur | A | Oui | Oui | Oui | L'état d'une tâche est toujours écrit (colonne État, résumé Android) ; la couleur ne fait que le doubler |
| 1.4.2 Contrôle du son | A | S. o. | S. o. | S. o. | Aucun son |
| 1.4.3 Contraste (minimum) | AA | Corrigé | Oui | Oui | `contrast.test.js` sur 17 couples par thème. « Terminé » sur une ligne choisie était à 4,03:1 en thème clair. Android : couleurs de Material 3, et ATF. Extension : `a11y.test.cjs` |
| 1.4.4 Redimensionnement du texte | AA | Corrigé | Oui | Oui | Le bureau n'avait aucun zoom (Tauri le coupe par défaut) : `zoomHotkeysEnabled`, Ctrl+ et Ctrl−. Distante : zoom du navigateur. Android : tailles en `sp`, qui suivent la taille de police du système. **Essai** : 200 % sur le bureau |
| 1.4.5 Texte sous forme d'image | AA | Oui | Oui | Oui | Aucun. Le QR code de l'accès à distance est accompagné de son adresse en texte |
| 1.4.10 Redistribution | AA | Oui | Oui | Oui | Disposition étroite par requêtes média : barre défilante, tableau en cartes, cibles de 44 px ; essayée sur téléphone avec l'interface distante (étape 3e). À 400 %, une fenêtre de 1 280 px fait 320 px CSS et prend cette disposition. **Essai** |
| 1.4.11 Contraste du contenu non textuel | AA | Corrigé | Oui | Corrigé | Les bords des champs, listes et boutons étaient à 1,6:1 (page) et 1,4:1 (extension) : couleurs `--control-border` et `--control`, à 3:1 au moins. Contour du focus de 4,8:1 à 9,7:1 |
| 1.4.12 Espacement du texte | AA | Oui | Oui | Oui | Hauteurs en `min-height`, textes qui passent à la ligne (`overflow-wrap`). Les colonnes du tableau coupent par points de suspension, avec le texte entier dans les Propriétés |
| 1.4.13 Contenu au survol ou au focus | AA | Oui | S. o. | Oui | Seules infobulles : les `title` natifs, gérés par le navigateur. Le menu s'ouvre au clic ou au clavier, et se ferme par Échap |
| 2.1.1 Clavier | A | Oui | Oui | Oui | Toutes les commandes au clavier : flèches, Entrée, Suppr, menu par Maj+F10 ou la touche Menu, Ctrl+N (`live.test.js`). Le glisser-déposer de liens a son équivalent : « Ajouter ». Android : navigation au clavier de Compose |
| 2.1.2 Pas de piège au clavier | A | Oui | Oui | Oui | Boîtes natives `dialog` (Échap) ; le menu se ferme par Échap et rend le focus (`live.test.js`) |
| 2.1.4 Raccourcis à caractère unique | A | Oui | S. o. | S. o. | Aucun raccourci d'un seul caractère. Suppr et les flèches n'agissent que dans le tableau qui a le focus |
| 2.2.1 Réglage du délai | A | Oui | Oui | Oui | Aucun délai imposé. Une session distante dure 30 jours (exception des plus de 20 heures). Les messages « Enregistré » qui s'effacent ne demandent aucune action |
| 2.2.2 Mettre en pause, arrêter, masquer | A | Oui | Oui | Oui | Seul mouvement : le reflet des barres de progression. Il s'arrête avec « Réduire les animations » (`prefers-reduced-motion`) et n'existe pas sous Linux. L'avancement chiffré est une mise à jour d'un état demandé par l'utilisateur |
| 2.3.1 Pas plus de trois flashs | A | Oui | Oui | Oui | Aucun flash |
| 2.4.1 Contourner des blocs | A | Corrigé | S. o. | Oui | Repères (voir 1.3.1) ; titre `h1` caché en tête de la fenêtre, puis titres `h2` |
| 2.4.2 Titre de page | A | Oui | Oui | Oui | `<title>` ; chaque boîte nommée par son titre ; titres d'écran Android dans la barre du haut |
| 2.4.3 Parcours du focus | A | Oui | Oui | Oui | Le focus entre dans chaque boîte et revient à son origine : Propriétés et menu dans `live.test.js`, autres boîtes par le même code |
| 2.4.4 Fonction du lien | A | Oui | S. o. | Oui | Liens au texte explicite (« Options », pages d'aide) |
| 2.4.5 Accès multiples | AA | S. o. | S. o. | S. o. | Application à un seul écran principal : l'exception du critère (étape d'un processus) s'applique aux boîtes |
| 2.4.6 En-têtes et étiquettes | AA | Oui | Oui | Oui | Chaque champ a son étiquette visible (axe) ; rubriques titrées |
| 2.4.7 Visibilité du focus | AA | Oui | Oui | Oui | `:focus-visible` à 2 px partout, ligne active du tableau comprise ; contraste vérifié (1.4.11). Android : indication de Compose |
| 2.4.11 Focus non masqué (minimum) | AA | Corrigé | Oui | Oui | L'en-tête du tableau reste collé en haut : une ligne atteinte en remontant pouvait passer dessous. `scroll-padding-top` la fait s'arrêter dessous. **Essai** |
| 2.5.1 Gestes de pointeur | A | Oui | Oui | Oui | Aucun geste à plusieurs points ni à tracé |
| 2.5.2 Annulation du pointeur | A | Oui | Oui | Oui | Les actions partent au relâchement (clics natifs) |
| 2.5.3 Nom accessible dans l'étiquette | A | Oui | Oui | Oui | Les boutons à texte ont ce texte pour nom. Le bouton de ligne Android, icône seule, ajoute le nom du fichier à l'action |
| 2.5.4 Activation par le mouvement | A | S. o. | S. o. | S. o. | Aucune |
| 2.5.7 Mouvements de glissement | AA | Oui | Oui | Oui | Seul glisser : déposer un lien ou un `.torrent` sur la fenêtre, avec « Ajouter » pour équivalent. Le redimensionnement des champs de texte est natif et facultatif |
| 2.5.8 Taille de la cible (minimum) | AA | Corrigé | Corrigé | Oui | Page : la flèche de l'arborescence faisait 22 px de large, elle fait 24 px ; boutons de 28 px de haut ; cibles de 44 px en disposition étroite. Android : les choix des réglages faisaient 36 dp, ils font 48 dp (ATF) |
| 3.1.1 Langue de la page | A | Oui | Oui | Oui | `lang` suit la langue choisie (français ou anglais) ; Android suit la langue du système |
| 3.1.2 Langue d'un passage | AA | Oui | Oui | Oui | Noms de fichiers et adresses non traduits, sans langue propre ; aucun passage dans une autre langue |
| 3.2.1 Au focus | A | Oui | Oui | Oui | Le focus ne déclenche rien |
| 3.2.2 À la saisie | A | Oui | Oui | Oui | Un réglage changé s'applique sans changer d'écran ni de contexte |
| 3.2.3 Navigation cohérente | AA | Oui | Oui | S. o. | Barre d'outils et rubriques au même endroit partout |
| 3.2.4 Identification cohérente | AA | Oui | Oui | Oui | Mêmes libellés pour les mêmes fonctions (bureau et Android partagent leurs libellés) |
| 3.2.6 Aide cohérente | A | S. o. | S. o. | S. o. | Aucun mécanisme d'aide répété d'un écran à l'autre ; le guide viendra à l'étape 9 |
| 3.3.1 Identification des erreurs | A | Corrigé | Oui | Oui | Messages en texte avec `role="alert"`, focus sur le champ fautif (règles, miroirs, adresses, empreinte), qui porte l'erreur (`aria-invalid`, `aria-describedby` : A15, A17) ; Android : texte d'erreur sous le champ et messages en bas d'écran |
| 3.3.2 Étiquettes ou instructions | A | Oui | Oui | Oui | Étiquettes visibles, notes liées par `aria-describedby` (format attendu, exemples) |
| 3.3.3 Suggestion après une erreur | AA | Oui | Oui | Oui | Les messages disent quoi faire (ex. : « essayer l'adresse en ftp:// », format du jeton) |
| 3.3.4 Prévention des erreurs | AA | Oui | Oui | S. o. | Aucune transaction juridique ou financière. Ce qui efface des fichiers demande une confirmation (Supprimer, Retélécharger, clé SSH) |
| 3.3.7 Saisie redondante | A | Oui | Oui | Oui | Rien n'est redemandé dans un même parcours ; les champs gardent leur saisie après un refus |
| 3.3.8 Authentification accessible (minimum) | AA | Oui | S. o. | S. o. | Interface distante : un jeton collé (copier-coller permis), ou le QR code qui le porte ; aucun test cognitif |
| 4.1.2 Nom, rôle et valeur | A | Corrigé | Oui | Oui | axe : `aria-labelledby` interdit sur deux `<pre>` (devenus `role="region"`), titres et boutons vides dans le balisage avant le script. Android : ATF, rôles des choix et des interrupteurs |
| 4.1.3 Messages d'état | AA | Corrigé | Oui | Oui | `role="status"` et `role="alert"` ; zone `aria-live` des annonces après une action (miroir, empreinte, limite) ; dans la boîte ouverte, s'il y en a une (A14 à A16, essai NVDA). Android : messages en bas d'écran, lus par TalkBack |

## 4. Constats et corrections

| # | Défaut | Critère | Interface | Correction | Commit |
|---|---|---|---|---|---|
| A1 | Bords des champs, listes et boutons à 1,6:1 | 1.4.11 | page | Couleur `--control-border` (3,7:1 clair, 3:1 au moins sombre) | 960260a |
| A2 | « Terminé » sur une ligne choisie à 4,03:1 (thème clair) | 1.4.3 | page | Vert `#256b29` (5,1:1) | 960260a |
| A3 | Carte des segments (`role="img"`) sans nom | 1.1.1, 4.1.2 | page | Nommée par son titre | 960260a |
| A4 | `aria-labelledby` interdit sur les `<pre>` du diagnostic | 4.1.2 | page | `role="region"` | 960260a |
| A5 | Barre d'outils hors de tout repère, aucun `h1` | 1.3.1, 2.4.1 | page | `<header>`, `<h1>` caché | 960260a |
| A6 | Titre des Propriétés et bouton Pause vides avant le script | 4.1.2 | page | Textes par défaut | 960260a |
| A7 | Flèche de l'arborescence de 22 px de large | 2.5.8 | page | 24 px | 960260a |
| A8 | Bords des champs à 1,4:1 | 1.4.11 | extension | Couleur `--control` | fb10073 |
| A9 | Choix à bouton radio des réglages de 36 dp | 2.5.8 | Android | Hauteur de 48 dp au moins, composant `Choice` partout | 2d458f5 |
| A10 | Aucun zoom au bureau | 1.4.4 | bureau | `zoomHotkeysEnabled` (Ctrl+, Ctrl−, Ctrl+0) | c474b91 |
| A11 | Ligne active masquable par l'en-tête collé du tableau | 2.4.11 | page | `scroll-padding-top` | c474b91 |
| A12 | Bouton flottant « Ajouter » sans nom pour TalkBack, inopérant au double appui : la forme à `text` et `icon` de Material 3 retire le texte de l'arbre d'accessibilité | 4.1.2, 2.5.3 | Android | Forme à contenu libre : le texte fait le nom du bouton | 07a08d5 |
| A13 | « Réglages » et les deux « Retour » lus sur leur icône, qui ne réagit pas au double appui : le nom était sur un nœud intérieur non cliquable | 4.1.2 | Android | Nom posé sur le bouton lui-même, icône sans description | 07a08d5 |
| A14 | Erreurs et annonces muettes pendant qu'une boîte est ouverte : bandeau d'erreur et zone `aria-live` de la fenêtre sont inertes derrière une boîte modale (ajout refusé, miroir, empreinte, limite des Propriétés, action de fin) | 4.1.3, 3.3.1 | page | Messages dans la boîte au premier plan, zones de chaque boîte | 130103b |
| A15 | Ajout refusé lu en dernier, après tout le texte de la boîte, et sans l'état « non valide » : la boîte se refermait puis se rouvrait, et NVDA la relisait en entier | 3.3.1, 4.1.3 | page | La boîte « Ajouter » reste ouverte jusqu'au succès ; l'erreur est liée au champ (`aria-invalid`, `aria-describedby`) avant qu'il prenne le focus (aussi : empreinte et miroir des Propriétés, champ refusé d'une règle) | 130103b |
| A16 | Première annonce d'une boîte muette (« Empreinte retirée ») : zone `aria-live` créée au moment du message | 4.1.3 | page | Zones de chaque boîte créées au chargement | 130103b |
| A17 | Échec de suppression d'identifiants ou d'une clé SSH invisible : effacé aussitôt par la relecture de la liste | 3.3.1 | page | Liste relue, focus placé, puis le message | 130103b |
| A18 | Ligne du tableau muette sous Orca : sans nom propre, WebKitGTK ne le tire pas de ses cellules (Chromium, oui) | 4.1.2 | page (bureau Linux) | Chaque ligne nommée par ses cellules (`aria-labelledby`) | ce commit |
| T1 | Les tests axe des boîtes passaient sans rien vérifier | — | tests | `elementFromPoint` fourni, règle sautée = échec, défaut glissé | 960260a |
| T2 | A12 et A13 passaient les contrôles de l'ATF et l'arbre sémantique de Compose, qui les voyaient nommés | — | tests | Contrôle de l'arbre réel d'Android (`assertButtonsNamed`) | 07a08d5 |

## 5. Restes et risques connus

- **Couleur des barres de progression d'Android.** Le vert des tâches terminées
  (`#43A047`) est à environ 3,3:1 sur une surface blanche du thème clair : il tient le minimum de
  1.4.11 de justesse. L'état est toujours écrit à côté.
- **Zoom sous Linux.** Tauri l'imite par `set_webview_zoom` (pas de 20 %) : vérifié à
  l'essai Orca, correct.
- **Orca 46.1 et WebKitGTK.** Orca traite l'application comme une application WebKitGTK
  (script `WebKitGtk`), pas comme une page de navigateur. Constaté le 2026-11-18, sans
  correction possible dans l'application :
  - les lettres restent des raccourcis de navigation structurelle dans un champ : il faut
    Orca+Z avant de taper, et Orca les réactive quand le focus revient sur la boîte ;
  - les zones `aria-live` ne sont pas annoncées (le script ne sait que s'y déplacer) :
    « Empreinte retirée », « Miroir ajouté »… se taisent ; une erreur liée à un champ
    (A15) est lue, avec lui, quand il prend le focus. Les versions suivantes d'Orca ne sont
    pas vérifiées.
- **Ligne nommée (A18).** Le nom tiré des cellules change ce que lit NVDA sur une ligne :
  à revérifier avec NVDA.
- **Annonces des changements d'état.** Une tâche qui finit ou échoue n'est pas annoncée
  par la zone `aria-live`. C'est voulu (pas de bavardage pendant les téléchargements) ;
  la notification du système le dit. À revoir si les essais réels le demandent.
- **Contrastes mesurés par ATF.** Ils dépendent de ce que l'outil capture à l'écran ; les
  couleurs de Material 3 sont conformes par construction.
- **Ce que l'ATF ne voit pas.** L'ATF et l'arbre sémantique de Compose ne sont pas ce que
  lit TalkBack : A12 et A13 leur échappaient. Le contrôle de l'arbre réel les couvre
  désormais, mais seulement pour les noms des éléments cliquables ; l'ordre de lecture et
  les annonces restent vérifiés par les essais réels.
- **WebKitGTK et WebView2.** Leur arbre d'accessibilité peut différer de celui de Chromium
  que modèle axe : seuls les essais réels le disent.
- **Focus après un ajout.** Une fois, à l'essai NVDA, le focus a fini sur « Tous les
  téléchargements » au lieu du bouton « Ajouter », après un ajout qui suivait une erreur
  dans la même boîte. Non reproduit ensuite, avec ou sans l'option « afficher la fenêtre à
  l'ajout » ; rien changé. À surveiller.
- **Messages hors d'une boîte.** La bannière de l'action de fin reste dans la fenêtre :
  derrière une boîte, son texte est dit dans la boîte, mais « Annuler » ne se joint
  qu'après Échap.

## 6. Essais réels

Faits par l'utilisateur, sur des variantes d'essai : un moteur et une base d'essai, jamais
ses données. Résultats à reporter ici.

| Essai | Système | Lecteur | État |
|---|---|---|---|
| Bureau, variante d'essai | Windows 11 | NVDA | conforme le 2026-11-18, après correction d'A14 à A17 : fenêtre, tableau, menu, Propriétés, boîte « Ajouter » (erreur et succès), Options, zoom, annonces des Propriétés (empreinte, limite). Un premier essai sur l'application installée avait été fait le 2026-11-08 : fonctionnel |
| Bureau, variante d'essai sous WSLg | Ubuntu 24.04 | Orca 46.1 | conforme le 2026-11-18 après correction d'A18, avec les limites d'Orca 46 (section 5) : noms et rôles, tableau (lignes lues), menu par la touche Menu, Propriétés, boîte « Ajouter » (champ non valide), Options, zoom. Annonces `aria-live` non lues par Orca 46 |
| Préparation de l'essai Orca sous WSLg | — | — | Écueils de l'environnement, pas de l'application : NVDA (Windows) garde Insert, à quitter pendant l'essai ; Verr. Maj n'arrive que comme état, inutilisable comme touche Orca ; Maj+F10 retenu par la fenêtre WSLg (la touche Menu ouvre le menu) ; speech-dispatcher lancé par systemd sans `PULSE_SERVER` (bloquait Orca) ; `xkbcomp` manquant (`x11-xkb-utils`) ; crochet d'apport qui figeait Orca ; Orca en français par `language-pack-fr` et `language-pack-gnome-fr` |
| Application Android | Android 15 (tablette) | TalkBack | conforme le 2026-11-18, après correction d'A12 et A13 : liste, ajout, détails, clé SSH acceptée, réglages, boîte d'ajout |
| Extension | Windows 11, Edge ou Firefox | NVDA | à faire |
