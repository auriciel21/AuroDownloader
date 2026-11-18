# Organisation

## Files d'attente

Chaque téléchargement est dans une file. La « File principale » existe d'office ; elle fait
3 téléchargements à la fois, les autres attendent (« En attente »). D'autres files se créent
par la ligne de commande, avec leur nombre de téléchargements simultanés et leur propre limite
de débit :

```
auro queue create Nuit --max-active 2 --limit 2M
auro add <adresse> --queue Nuit
```

Les files apparaissent sous « Files d'attente », dans les catégories.

## Limites de débit

- **Globale** : en bas de la fenêtre, « Limite de débit globale », ou Options, Connexion.
- **D'un téléchargement** : menu « Limiter le débit… », ou ses Propriétés.
- **D'une file** : par la ligne de commande (`auro limit 1M --queue Nuit`).

Une limite changée vaut aussitôt, pour les téléchargements en cours aussi.

## Planification

« Planification » règle, pour une file :

- **Ne télécharger que pendant ces plages horaires** : des jours et des heures (« Ajouter une
  plage »), par exemple du lundi au vendredi de 23:00 à 07:00. Hors des plages, la file
  attend (« En pause (plage horaire) ») et repart seule à l'ouverture.
- **Quand la file n'a plus rien à télécharger** : Ne rien faire, Mettre l'ordinateur en veille
  ou Éteindre l'ordinateur. L'action est annoncée une minute avant, avec un bouton pour
  l'annuler.

## Règles de rangement

Options, Règles. Une règle range les téléchargements ajoutés **sans dossier choisi** (par
l'extension, les liens copiés, l'interface distante) :

- **Si** : types de fichier ou extensions, domaines du site, expression régulière sur
  l'adresse ou le nom du fichier (tous les critères remplis ; vides : toute tâche).
- **Alors** :
  - un **dossier** : chemin complet, ou relatif au dossier des téléchargements
    (`Vidéos/Conférences`), créé s'il manque ;
  - un **nom du fichier**, modèle avec `{nom}` (sans l'extension), `{ext}`, `{hôte}` et
    `{date}` (jour de l'ajout) : par exemple `{date} {nom}.{ext}` ;
  - une **commande à la fin du téléchargement** : un programme (chemin complet), ses
    arguments, un par ligne, et un délai. Variables : `{chemin}` (le fichier), `{dossier}`,
    `{nom}`, `{taille}`, `{sha256}`, `{id}`. Par exemple, un antivirus avec `{chemin}`. La
    commande ne passe jamais par un shell ; le téléchargement reste en « Finition » tant
    qu'elle tourne, puis échoue si elle échoue ou dépasse son délai (« Reprendre » la
    refait).

La première règle activée qui correspond s'applique ; « Monter » et « Descendre » changent
l'ordre. Un torrent ne prend que le dossier. Les règles ne se modifient que dans l'application
de l'ordinateur (pas par l'interface distante).

## Historique

« Historique » liste les téléchargements finis, même retirés de la liste : recherche par nom,
dossier ou site, « Exporter en CSV… », « Effacer l'historique… ».
