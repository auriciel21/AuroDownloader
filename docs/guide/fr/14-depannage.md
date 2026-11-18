# Dépannage

## Problèmes fréquents

| Ce que l'on voit | Que faire |
| --- | --- |
| « Nouvel essai bientôt » qui dure | Le réseau ou le serveur ne répond pas : AuroDownloader réessaie de lui-même. Vérifiez la connexion et le proxy (Options, Connexion). |
| « refusé par le serveur » | Le lien a peut-être expiré : « Actualiser l'adresse… » avec un lien neuf, pris sur la page d'origine. |
| Le serveur demande de se connecter | Identifiants du site (Options, Identifiants de sites), ou passez par l'extension, qui transmet les cookies. |
| « certificat refusé » | Le certificat HTTPS du serveur n'est pas valable : AuroDownloader ne télécharge pas sans lui. |
| « empreinte différente de celle attendue » | Le fichier reçu n'est pas celui annoncé : vérifiez l'empreinte, ou retéléchargez. |
| « disque plein » | Faites de la place : le téléchargement repart seul. |
| « fichier distant modifié » | Le fichier a changé sur le serveur pendant le téléchargement : « Retélécharger… ». |
| L'extension ne prend rien | Fenêtre de l'icône, Réglages, « Vérifier » ; désactivez l'extension d'IDM ; sous Linux, préférez le paquet `.deb`. |
| Une vidéo refusée | Elle est protégée (DRM), ou yt-dlp ne connaît pas encore le site : attendez sa mise à jour de lui-même. |
| Les torrents ne démarrent pas | Si une interface réseau (VPN) est choisie (Options, BitTorrent), elle doit être là. |

## Diagnostic

Options, À propos, **Exporter un diagnostic…** : un résumé de l'état du moteur et ses
journaux récents, **nettoyés** (sans adresses, noms de fichiers, en-têtes ni identifiants),
montrés avant d'être enregistrés dans une archive `.zip`, à joindre à un signalement. Pour un
problème qui se reproduit, réglez d'abord « Détail des journaux » sur « Détaillé, pour un
dépannage », reproduisez-le, puis exportez.

## Où sont les données

- La liste des téléchargements, l'historique et les réglages du moteur sont dans une base
  (`auro.db`), avec les journaux (`logs`) : `%LOCALAPPDATA%\AuroDownloader` sous Windows,
  `~/.local/share/aurodownloader` sous Linux.
- Les mots de passe, cookies et jetons sont dans le coffre du système, jamais dans la base.

## Vie privée

AuroDownloader n'envoie aucune télémétrie. Il ne contacte que :

- les serveurs des fichiers que vous téléchargez (et leurs miroirs) ;
- les pairs des torrents ;
- GitHub, une fois par jour au plus, pour les mises à jour (désactivable) ;
- les publications officielles de yt-dlp et ffmpeg, si vous les installez ;
- les domaines déclarés par les plugins que vous installez.

## Signaler un problème

Sur la page des [tickets](https://github.com/aurodownloader/aurodownloader/issues), avec la
version (Options, À propos), le système, ce qui se passe, et le diagnostic si possible. Une
faille de sécurité se signale en privé, par les
[avis de sécurité](https://github.com/aurodownloader/aurodownloader/security/advisories/new).
