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

« Planification » règle, pour la file choisie dans « File d'attente » :

- **Ne télécharger que pendant ces plages horaires** : des jours et des heures (« Ajouter une
  plage »), par exemple du lundi au vendredi de 23:00 à 07:00. Hors des plages, la file
  attend (« En pause (plage horaire) ») et repart seule à l'ouverture.
- **Quand la file n'a plus rien à télécharger** : Ne rien faire, Mettre l'ordinateur en veille
  ou Éteindre l'ordinateur. L'action est annoncée une minute avant, avec un bouton pour
  l'annuler.

« Enregistrer » applique la planification ; une plage ouverte démarre aussitôt les
téléchargements en attente de la file.

## Règles de rangement

Options, Règles. Une règle range les téléchargements ajoutés **sans dossier choisi** (par
l'extension, les liens copiés, l'interface distante) :

La boîte « Règle de rangement » a un **Nom** et une case **Activée**, puis :

- **Si** (tous les critères remplis ; vides : toute tâche) : **Types de fichier** (Archives,
  Documents, Images, Musique, Programmes, Vidéos…) **ou extensions** (`iso, pdf`, un type
  coché ou une extension listée suffit), **Domaines
  du site**, **Adresse (expression régulière)**, **Nom du fichier (expression
  régulière)**.
- **Alors** :
  - un **dossier** : chemin complet, ou relatif au dossier des téléchargements
    (`Vidéos/Conférences`), créé s'il manque ;
  - un **nom du fichier**, modèle avec `{nom}` (sans l'extension), `{ext}`, `{hôte}` et
    `{date}` (jour de l'ajout) : par exemple `{date} {nom}.{ext}` ;
  - une **Commande à la fin du téléchargement** : « Programme (chemin complet) »,
    « Arguments, un par ligne », et « Délai (secondes) ». Variables : `{chemin}` (le fichier), `{dossier}`,
    `{nom}`, `{taille}`, `{sha256}`, `{id}`. Par exemple, un antivirus avec `{chemin}`. La
    commande ne passe jamais par un shell ; le téléchargement reste en « Finition » tant
    qu'elle tourne, puis échoue si elle échoue ou dépasse son délai (« Reprendre » la
    refait).

La première règle activée qui correspond s'applique ; « Monter » et « Descendre » changent
l'ordre. Un torrent ne prend que le dossier. Les règles ne se modifient que dans l'application
de l'ordinateur (pas par l'interface distante).

## Historique

« Historique » liste les téléchargements finis, même retirés de la liste, les plus récents
d'abord : Fini le, Nom, Taille, État (terminé, en échec, annulé), Site.

- **Rechercher (nom, dossier ou site)** : la liste se filtre pendant la frappe.
- **Afficher plus** : les entrées suivantes, par pages.
- **Exporter en CSV…** : la liste, à ouvrir dans un tableur.
- **Effacer l'historique…** : après confirmation (« Effacer »), l'historique est vidé ; les
  fichiers restent sur le disque.
