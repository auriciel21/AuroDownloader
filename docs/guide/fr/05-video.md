# Vidéos

Sous Windows et Linux, AuroDownloader télécharge la vidéo d'une page web, par deux programmes
libres : **yt-dlp** (qui lit la page) et **ffmpeg** (qui assemble l'image et le son). Il ne
télécharge pas les vidéos protégées (DRM).

## Installer les outils

La première fois, la boîte « Ajouter une vidéo » (bouton « Vidéo ») propose « Installer yt-dlp
et ffmpeg » : ils sont téléchargés depuis leurs publications officielles, leur empreinte ou
leur signature est vérifiée, et ils occupent près de 400 Mo. Rien n'est installé sans ce clic.

yt-dlp se met à jour lui-même, au plus une fois par jour (Options, Vidéo, « Mettre à jour
yt-dlp de lui-même »), car les sites changent souvent.

## Télécharger une vidéo

1. « Vidéo », collez l'adresse dans « Adresse de la page de la vidéo », puis « Analyser ».
   Les sites sans DRM conviennent : PeerTube, Wikimedia Commons, Internet Archive… La page est
   lue par yt-dlp.
2. AuroDownloader montre le titre, la durée, le site et les qualités disponibles, avec leur
   taille estimée. Choisissez la **Qualité**, ou dépliez « Tous les formats » pour un choix
   précis : chaque format avec son Contenu (image, son, ou les deux), son Image (définition),
   sa Taille et son Protocole. Choisissez aussi le **Conteneur** (« Automatique » par
   défaut).
3. **Sous-titres** : cochez les langues voulues ; ils sont gardés à côté de la vidéo, au
   format du site. Une langue manquante est signalée à la fin.
4. « Enregistrer dans » et, si besoin, un « Sous-dossier », créé dans le dossier choisi.
5. « Démarrer » ou « Télécharger plus tard ».

La vidéo passe par les mêmes files, limites de débit et plages horaires que les autres
téléchargements, et reprend de même après une coupure.

Depuis le navigateur, l'[extension](09-extension.md) fait la même chose en un clic.
