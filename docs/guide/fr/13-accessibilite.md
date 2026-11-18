# Accessibilité

AuroDownloader se sert entièrement au clavier, et a été essayé avec NVDA (Windows), Orca
(Linux) et TalkBack (Android).

## Clavier, dans la fenêtre

| Touches | Effet |
| --- | --- |
| F1 | Ce guide (aussi dans Options, À propos) |
| Ctrl+N ou Ctrl+L | Ajouter |
| Tab, Maj+Tab | Barre d'outils, catégories, tableau |
| Flèches haut et bas, Page préc., Page suiv., Début, Fin | Ligne précédente, suivante… |
| Maj+flèches | Étendre la sélection |
| Ctrl+Espace | Ajouter ou retirer une ligne de la sélection |
| Ctrl+A | Tout sélectionner |
| Entrée | Propriétés |
| Suppr | Supprimer… |
| Maj+F10 ou touche Menu | Menu contextuel |
| Échap | Fermer un menu ou une boîte ; le focus revient d'où il venait |
| Flèches gauche et droite | Rubrique suivante ou précédente des Options |
| Ctrl+plus, Ctrl+moins, Ctrl+0 | Agrandir, réduire, taille normale |

Dans la boîte « Ajouter », Entrée démarre, Maj+Entrée passe à la ligne.

## Lecteurs d'écran

- Les messages (ajout refusé, empreinte retirée, téléchargement terminé…) sont annoncés ; une
  erreur liée à un champ est lue avec lui.
- **Orca** : la version 46 traite l'application comme une page WebKitGTK ; dans un champ, les
  lettres servent à se déplacer tant que la navigation structurelle est active : Orca+Z
  la coupe avant de taper. Orca 46 n'annonce pas non plus les messages d'état de la page.

Le détail de la revue d'accessibilité (WCAG 2.2, niveaux A et AA) est dans
[`docs/ACCESSIBILITE.md`](../../ACCESSIBILITE.md).
