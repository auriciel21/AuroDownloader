# Prototypes de l'application de bureau : Qt Quick et Tauri

Phase 2, étape 5 (SPEC §4.3) : deux prototypes au périmètre identique, mesurés de la même
façon, pour choisir la technologie de l'application de bureau. Ils ne font pas partie du
produit et ne sont pas dans l'espace de travail Cargo.

## Périmètre commun

Chaque prototype :

1. se connecte à un `aurod` déjà lancé, sur le point d'accès donné par `--endpoint`
   (jamais le moteur réel de l'utilisateur pendant les essais) ;
2. ouvre la session (`engine.hello`), s'abonne aux sujets `tasks` et `progress`
   (`events.subscribe`), puis charge l'état complet (`sync.snapshot`) ;
3. affiche les tâches : nom, état, barre d'avancement, octets reçus et taille, débit,
   temps restant, mis à jour par `task.added`, `task.stateChanged` et `task.progress` ;
4. ajoute une adresse (`task.add`, dossier donné par `--dest`), met en pause, reprend et
   annule une tâche (`task.pause`, `task.resume`, `task.cancel`) ;
5. reste dans la zone de notification quand on ferme la fenêtre ; le menu de l'icône
   rouvre la fenêtre ou quitte (F-36) ;
6. suit le thème clair ou sombre du système (F-36) ;
7. a ses textes en français, traduits en anglais (N-10) ; `--lang en` force l'anglais ;
8. s'utilise au clavier, et chaque commande porte un nom accessible pour les lecteurs
   d'écran (N-09) ;
9. écrit sur sa sortie standard `PREMIERE_IMAGE <ms Unix>` à la première image affichée
   avec les tâches, pour mesurer le démarrage.

Hors périmètre : démarrer `aurod` s'il ne tourne pas (même logique dans les deux,
reprise de la CLI), réglages, files, limites, installateurs.

## Mesures

Même machine, même `aurod` d'essai, mêmes tâches, outils alternés :

- démarrage : du lancement à `PREMIERE_IMAGE`, médiane de 5 ;
- mémoire : somme de la mémoire privée de tous les processus de l'application (pour
  Tauri, WebView2 compris), au repos avec 100 tâches affichées, puis avec 20 tâches en
  cours ;
- processeur : temps processeur de tous ses processus sur 60 s, avec 20 tâches en cours
  (une notification d'avancement groupée toutes les 500 ms) ;
- taille à distribuer : dossier déployé (Qt : `windeployqt`) ou exécutable (Tauri,
  WebView2 étant fourni par Windows 11) ;
- accessibilité : arbre UI Automation (noms et rôles des commandes) ; essai réel avec NVDA
  par l'utilisateur ;
- code : lignes écrites, langages, réutilisation du code Rust du projet.

Linux : mesuré à part, dans une distribution WSL d'essai (section « Linux, WSLg »).

## Résultats du 2026-11-06 (Windows 11)

Machine : Intel Core i7-3930K (2011), Windows 11 Pro, Qt 6.10.2 (MSVC 2022, style
FluentWinUI3 par défaut), Tauri 2 avec WebView2 156. `aurod` d'essai : 100 tâches, dont
20 en cours (limite globale de 1 Mio/s), notification d'avancement groupée toutes les
500 ms. Processeur : cycles de tous les processus de l'application
(`QueryProcessCycleTime`), en part d'un cœur ; mémoire : mémoire privée cumulée.
Applications alternées, 5 passages, médianes (`proto-bench.ps1`, hors dépôt).

| Mesure | Qt Quick | Tauri |
| --- | --- | --- |
| Démarrage jusqu'à la première image avec les tâches | 831 ms | 1 011 ms |
| Processus | 1 | 7 (WebView2 : principal, rendu, GPU, utilitaires) |
| Mémoire privée, fenêtre visible | 99,6 Mio | 179,8 Mio |
| Mémoire privée, fenêtre repliée | 99,5 Mio | 187,0 Mio |
| Processeur, fenêtre visible | 0,26 % | 3,36 % |
| Processeur, fenêtre repliée, avancement transmis à la page | 0,26 % | 3,41 % |
| Processeur, fenêtre repliée, avancement suspendu (voir plus bas) | — | 0,39 % (un passage) |
| Processeur au repos, sans moteur, visible / repliée | 0,21 / 0,16 % | 0,45 / 0,14 % |
| À distribuer | 67,9 Mio, 1 324 fichiers (`windeployqt`, non trié) | 8,1 Mio, un exécutable (WebView2 fourni par Windows 11) |
| Première compilation | CMake, MSVC et le SDK Qt (installateur, plusieurs Gio) | `cargo` seul, 9 min 22 s |
| Code écrit (lignes non vides) | 543 : C++ et QML | 452 : Rust, JavaScript, HTML, CSS |
| Réutilisation du code Rust du projet | aucune : client JSON-RPC refait en C++ (`QLocalSocket`) | `auro_engine::rpc::local` et `paths` repris tels quels |
| Traduction | outils Qt (`lupdate`, `.ts`, pluriels) | dictionnaire écrit à la main |
| Accessibilité (arbre UI Automation) | tout est nommé (champ, liste, lignes, barres, boutons) ; seules les lignes affichées existent (9 sur 100) | tout est nommé, les 100 lignes existent ; arbre construit à la demande, quand un lecteur d'écran le lit |

**Coût de Tauri pendant le téléchargement.** Il vient de la livraison de chaque
notification à la page et de son affichage : 1,5 à 2 % dans le processus de rendu,
le reste dans les processus principal et GPU de WebView2. Ne pas redessiner une page
cachée (`document.hidden`) n'a rien changé. En revanche, quand la partie Rust cesse de
transmettre l'avancement pendant que la fenêtre est repliée, puis fait recharger l'état
complet à son retour, le coût replié tombe à 0,39 % (un passage de 20 s). Ce
rechargement à la réouverture n'a pas été essayé à la main (clic sur l'icône).

**Non mesuré :** Windows 10 (WebView2 à installer s'il manque), déploiement Qt trié
(seuls les modules et le style utilisés). L'essai réel avec NVDA a été fait plus tard, le
2026-11-08, par l'utilisateur, sur l'application Tauri installée (`apps/desktop`) :
fonctionnel.

## Résultats du 2026-11-06 (Linux, WSLg)

Même machine, distribution d'essai Ubuntu 24.04 sous WSL 2 avec WSLg (affichage par
XWayland, sans accélération graphique : OpenGL en logiciel, llvmpipe, pour les deux).
Qt 6.10.2 officiel (Ubuntu 24.04 ne fournit que Qt 6.4, et le prototype demande 6.8),
WebKitGTK 2.52.6 de la distribution. Mêmes sources, compilées sans modification. Même
`aurod` d'essai (socket Unix), mêmes tâches. Processeur : `utime` + `stime` de tous les
processus de l'application, en part d'un cœur ; mémoire : USS (pages privées) cumulée.
Fenêtre fermée par `WM_DELETE_WINDOW` (WSLg n'a pas de zone de notification : l'application
reste en mémoire, sans icône). Applications alternées, 5 passages de 30 s, médianes.

| Mesure | Qt Quick | Tauri | Tauri sans composition |
| --- | --- | --- | --- |
| Démarrage jusqu'à la première image avec les tâches | 369 ms | 1 391 ms | 1 193 ms |
| Processus | 1 | 3 (principal, rendu, réseau) | 3 |
| Mémoire USS, fenêtre visible | 177,8 Mio | 309,3 Mio | 168,8 Mio |
| Processeur, fenêtre visible | 0,07 % | 12,8 % | 2,0 % |
| Processeur, fenêtre fermée, avancement suspendu | 0,03 % | 0,17 % | 0,13 % |
| Arbre d'accessibilité AT-SPI (ce que lit Orca) | 0 sans nom sur 47 ; 11 lignes sur 100 | 0 sans nom sur 402 ; 100 lignes sur 100 | — |

« Sans composition » : `WEBKIT_DISABLE_COMPOSITING_MODE=1`. Par défaut, WebKitGTK compose
la page en OpenGL ; sans accélération, cet OpenGL tourne en logiciel et le processus de
rendu prend 10 à 12 % d'un cœur. Sans composition, la page est dessinée directement par
le processeur, ce qui coûte bien moins pour une liste mise à jour toutes les 500 ms ; la
mémoire tombe au niveau de Qt. `WEBKIT_DISABLE_DMABUF_RENDERER=1` donne un résultat
voisin (3,5 %, un passage). Sur un poste Linux avec accélération graphique, le mode par
défaut n'a pas été mesuré.

**Constats.**

- Les deux s'affichent correctement (captures vérifiées) et parlent au moteur par le
  socket Unix sans changement de code : `auro_engine::rpc::local` sert tel quel sous Linux.
- Sans bus D-Bus de session, Tauri reste bloqué avant d'ouvrir sa fenêtre (cause non
  établie ; l'icône de la zone de notification et l'accessibilité passent par D-Bus) ; Qt
  démarre. Un bureau Linux a toujours ce bus, mais l'application
  devra le vérifier plutôt que se bloquer.
- Zone de notification : ni GNOME sans extension ni WSLg n'en ont. Fermer la fenêtre ne doit
  la replier que si une icône existe (Qt : `QSystemTrayIcon::isSystemTrayAvailable` ; Tauri
  ne le dit pas, il faudra le déterminer par D-Bus, `org.kde.StatusNotifierWatcher`).
- Distribution : Tauri dépend du WebKitGTK du système (paquet `.deb` ou `.rpm`, ou
  AppImage et Flatpak qui l'embarquent) ; Qt 6.8 et plus n'est pas dans Ubuntu 24.04 LTS,
  l'application devrait embarquer son Qt.
- Accessibilité : nom de ligne mieux composé par Qt (« gros.bin, Paused, 0 B of ? ») ; Tauri
  concatène les textes de la ligne, à régler par un `aria-label`.

**Non mesuré sous Linux :** poste natif avec accélération graphique, Wayland natif, essai
réel avec Orca, menu de la zone de notification (KDE).

**Défaut du moteur trouvé pendant ces essais :** deux tâches du même nom préparées en
même temps partageaient un `.part`. Corrigé à part (SPEC §7.1, commit
« Noms de fichier : deux tâches ne partagent jamais un .part »).
