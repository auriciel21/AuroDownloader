# Captures d'écran du guide (étape 9e)

Liste des captures à faire par le mainteneur, sur la **variante d'essai** (moteur, base et
préférences d'essai : jamais l'application ni les données réelles), puis à intégrer au guide.
Chaque capture se fait deux fois : interface en français pour `docs/guide/fr`, en anglais pour
`docs/guide/en` (Options, Général, Langue).

## Avant de capturer

- Fenêtre à sa taille d'origine (1120 × 680), thème clair, zoom normal (Ctrl+0).
- Dossier des téléchargements d'essai au nom neutre (par exemple `C:\Essai\Téléchargements`),
  pour qu'aucun chemin personnel n'apparaisse.
- Rien de personnel à l'écran : ni nom d'utilisateur, ni adresse privée, ni notification, ni
  jeton de l'accès à distance (le QR code et le jeton en contiennent un : ne pas les capturer).
- Fichiers publics seulement (miroir Debian, Framatube, Internet Archive).
- Format PNG, la fenêtre seule (Alt+Impr. écran sous Windows).

## Liste

Fichiers : `docs/guide/images/fr/<nom>.png` et `docs/guide/images/en/<nom>.png`, même nom dans
les deux langues.

| Nom | Ce qu'elle montre | Où elle va | Texte alternatif (fr / en) |
|---|---|---|---|
| `01-fenetre.png` | La fenêtre principale avec 5 ou 6 téléchargements dans des états variés (en cours avec avancement, en pause, terminé, en échec, en attente) | 02, « La fenêtre » | La fenêtre d'AuroDownloader : barre d'outils, catégories à gauche, tableau des téléchargements / The AuroDownloader window: toolbar, categories on the left, table of downloads |
| `02-ajouter.png` | La boîte « Ajouter des téléchargements », une adresse saisie, le dossier visible | 02, « Un premier téléchargement » | La boîte Ajouter des téléchargements, avec une adresse / The Add downloads box, with an address |
| `03-proprietes.png` | Les Propriétés d'un téléchargement en cours : statut, connexions, carte des segments | 03, « Propriétés » | Les Propriétés d'un téléchargement en cours, avec la carte des segments / The Properties of a running download, with the segment map |
| `04-torrent.png` | Les Propriétés d'un torrent avec la liste de ses fichiers à cocher (netinst Debian) | 04, « Choisir les fichiers » | Les fichiers d'un torrent, à cocher / A torrent's files, to tick |
| `05-video.png` | « Ajouter une vidéo » après « Analyser » sur une page Framatube : titre, qualités, sous-titres | 05, « Télécharger une vidéo » | La boîte Ajouter une vidéo : titre, qualités et sous-titres / The Add a video box: title, qualities and subtitles |
| `06-liens.png` | « Analyser des liens » sur une page du miroir Debian, filtres et liens cochés | 06, « Tous les fichiers d'une page » | Les liens d'une page, filtrés et cochés / The links of a page, filtered and ticked |
| `07-planification.png` | « Planification » avec une plage du lundi au vendredi, 23:00 à 07:00, et une action de fin | 07, « Planification » | La planification d'une file : plages horaires et action de fin / A queue's schedule: time slots and end action |
| `08-options.png` | Options, Général, rubrique « Mises à jour » visible | 08, « Général » | Les Options, rubrique Général / The Options, General section |
| `09-extension.png` | La fenêtre de l'icône de l'extension sur une page Framatube : qualité, sous-titres, Télécharger | 09, « Ce qu'elle fait » | La fenêtre de l'extension sur une page de vidéo / The extension's window on a video page |
| `10-android-liste.png` | L'application Android : la liste avec deux ou trois téléchargements | 10, « La liste et les détails » | La liste des téléchargements sur Android / The download list on Android |
| `11-android-details.png` | L'application Android : les détails d'un téléchargement | 10, « La liste et les détails » | Les détails d'un téléchargement sur Android / The details of a download on Android |

Les captures Android se font sur l'appareil d'essai (application aux données d'essai,
`pm clear` ensuite), sans la barre de notifications (elle peut montrer des notifications
privées).

## Intégration

Une fois les fichiers déposés, Claude les ajoute au guide sous le titre indiqué
(`![texte alternatif](../images/fr/01-fenetre.png)`), dans les deux langues. La copie du
guide dans l'application reste sans image : `scripts/guide.mjs` y remplace une image par son
texte alternatif, pour ne pas alourdir l'application.
