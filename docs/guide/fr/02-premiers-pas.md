# Premiers pas

## La fenêtre

- **Barre d'outils**, en haut : Ajouter, Vidéo, Liens, Reprendre, Arrêter, Tout reprendre,
  Tout arrêter, Supprimer, Retirer les finis, Propriétés, Planification, Historique, Options,
  Aide. « Aide » ouvre un menu : Guide de l'utilisateur (F1), Raccourcis clavier, À propos.
- **Catégories**, à gauche : Tous les téléchargements, Incomplets, Terminés, chacun avec ses
  types de fichier (Archives, Documents, Images, Musique, Programmes, Vidéos, Autres), puis
  les files d'attente. Le nombre de téléchargements est à côté de chaque catégorie ; un clic
  sur une catégorie ne montre qu'elle dans le tableau.
- **Tableau**, au centre : Nom du fichier, Taille, Avancement, Statut, Temps restant, Débit,
  Ajouté le. Un clic sur un titre de colonne trie le tableau.
- **Barre d'état**, en bas : ce qui se passe (téléchargements en cours, plages horaires,
  lignes sélectionnées), et la **Limite de débit globale** : « Aucune limite », ou un palier de
  128 Kio/s à 100 Mio/s, qui vaut aussitôt pour tous les téléchargements.

Un fichier `.torrent` ou Metalink glissé sur la fenêtre ouvre « Ajouter des téléchargements »,
déjà rempli.

Au premier lancement, AuroDownloader vous dit qu'il cherche ses mises à jour une fois par jour
au plus ; « Ne pas rechercher » coupe cette recherche, « D'accord » la garde.

## Un premier téléchargement

1. Copiez l'adresse d'un fichier, par exemple `https://deb.debian.org/debian/README`.
2. « Ajouter » (ou Ctrl+N), collez l'adresse dans « Adresses ou liens magnet, un par
   ligne ».
3. « Enregistrer dans » montre le dossier : par défaut, votre dossier Téléchargements ;
   « Parcourir… » en choisit un autre.
4. « Démarrer ». Le téléchargement apparaît dans le tableau, avec son avancement.

« Télécharger plus tard » l'ajoute en pause : « Reprendre » le lancera.

## Ce que l'on peut faire d'un téléchargement

Sélectionnez une ligne (plusieurs avec Ctrl ou Maj), puis la barre d'outils, ou un clic droit
pour le menu : Ouvrir, Afficher dans le dossier, Retélécharger…, Déplacer/Renommer…,
Actualiser l'adresse…, Reprendre, Arrêter, Limiter le débit…, Supprimer…, Propriétés.

- **Arrêter** met en pause ; **Reprendre** repart là où il s'était arrêté.
- **Supprimer…** retire de la liste ; pour un téléchargement terminé, la case « Supprimer
  aussi les fichiers terminés du disque » efface aussi le fichier.
- **Retirer les finis** retire de la liste les téléchargements terminés et annulés ; leurs
  fichiers restent sur le disque, et l'[historique](07-organisation.md#historique) les garde.
- Un double clic, ou Entrée, ouvre les **Propriétés**.

Un fichier terminé s'ouvre avec le programme du système ; par sécurité, AuroDownloader
n'ouvre jamais un programme ni un paquet d'installation : « Afficher dans le dossier » le
montre.

## Fermer la fenêtre

Fermer la fenêtre la replie dans la zone de notification (à côté de l'horloge) : les
téléchargements continuent. Un clic sur l'icône de la zone de notification rouvre la fenêtre ;
son menu (clic droit) propose « Ouvrir AuroDownloader », « Quitter (les téléchargements
continuent) » et « Quitter et arrêter les téléchargements ». Le moteur, qui fait les
téléchargements, est un programme à part : il continue après « Quitter », et l'application le
retrouve à son prochain lancement.
Ce comportement se règle dans les [Options](08-options.md#général).

Au redémarrage de l'ordinateur, les téléchargements inachevés reprennent quand AuroDownloader
se relance ; « Démarrer avec la session » le fait de lui-même.
