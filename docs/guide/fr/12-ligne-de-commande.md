# Ligne de commande

`auro` pilote le même moteur que l'application : un téléchargement ajouté par `auro` apparaît
dans la fenêtre, et inversement. Il est installé avec l'application (à côté d'elle sous
Windows, dans le chemin sous Linux avec le paquet `.deb`). Si le moteur ne tourne pas, `auro`
le démarre.

Chaque commande a son aide (`auro add --help`) et, sous Linux, sa page de manuel
(`man auro`, `man auro-add`, `man aurod`).

## Commandes courantes

```
auro get https://deb.debian.org/debian/README -o .   # télécharge et suit l'avancement
auro add <adresse>… --paused                         # ajoute sans attendre (en pause)
auro list                                            # liste
auro watch                                           # suit ce qui tourne
auro pause <id>… ; auro resume <id>…
auro remove <id>… ; auro restart <id>… --delete-files
auro move <id> -o <dossier> -n <nom>
auro refresh <id> <nouvelle-adresse>
```

Sans `-o`, `auro add` télécharge dans le dossier courant ; `--auto` laisse les
[règles de rangement](07-organisation.md#règles-de-rangement) choisir.

## Plus loin

```
auro add <adresse> --checksum sha256:<hex>          # empreinte attendue
auro get <adresse> --mirror <adresse>…              # miroirs
auro metalink <fichier.meta4> ; auro get <fichier.meta4> --select 0,2
auro add fichier.torrent ; auro files <id> --select 0,2-4
auro links <page> -e iso,zip --min-size 10M --add -o .
auro site <page> -o . --depth 2 ; auro site <page> -e pdf   # copie hors ligne
auro video info <page> ; auro video get <page> --subs fr,en
auro limit 2M ; auro limit --upload 500K ; auro limit 1M --queue Nuit
auro schedule set --days 1-5 --from 23:00 --to 07:00 --then sleep
auro login set <site> -u <nom> ; auro hostkey list
auro proxy manual socks5://hôte:1080 -u moi ; auro proxy system
auro history [texte] [--csv]
auro config ; auro config torrent.ratioLimit 1.5
auro diagnostic --save diag.zip
auro engine stop
```

## Codes de sortie

`auro` rend 0 quand tout est fait, 1 après une erreur ou quand une partie a été refusée
(par exemple un fichier d'un Metalink, les autres étant ajoutés), 2 pour une commande
mal écrite, et 130 quand Ctrl-C arrête un suivi (`get` met alors ses tâches en pause).
Une adresse affichée ne montre jamais son mot de passe.

## Le moteur seul

`aurod` est le moteur, sans fenêtre. Pour un serveur ou un NAS, il se lance seul, et l'on
s'en sert par `auro` ou par l'[accès à distance](11-acces-distant.md).
