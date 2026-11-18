# Téléchargements

## Ajouter

La boîte « Ajouter des téléchargements » prend :

- des **adresses** `http://`, `https://`, `ftp://`, `ftps://` ou `sftp://`, une par ligne
  (plusieurs à la fois) ;
- des **liens magnet** (voir [Torrents](04-torrents.md)) ;
- des **fichiers `.torrent` ou Metalink** (« Choisir des fichiers .torrent ou Metalink… », ou
  glissés sur la fenêtre).

Champs facultatifs :

- **Miroirs**, un par ligne : d'autres adresses du même fichier. Chacun est vérifié (même
  taille, mêmes octets) avant de servir, et le téléchargement se répartit entre eux selon leur
  débit ; un miroir en panne est écarté.
- **Empreinte attendue** : SHA-256, SHA-1 ou MD5 (`sha256:…` ou la valeur seule). Le fichier
  ne prend son nom qu'une fois vérifié ; sinon, le téléchargement échoue avec « empreinte
  différente de celle attendue » et le fichier reste sous son nom provisoire.
- **Enregistrer dans** : le dossier.

Si l'adresse est refusée, la boîte reste ouverte et dit pourquoi, le curseur dans le champ en
cause.

Une page d'un hébergeur de fichiers pris en charge par un [plugin](08-options.md#plugins) est
lue par lui ; « Ajouter ces pages telles quelles, sans plugin » l'évite.

## Statuts

En attente (file pleine, ou plage horaire fermée), En cours, En pause, Nouvel essai bientôt
(coupure du réseau ou du serveur : AuroDownloader réessaie de lui-même), Vérification de
l'empreinte, Finition (la commande d'une [règle](07-organisation.md#règles-de-rangement)
tourne), Terminé, Échec (la raison est dite), Annulé, Partage (torrent).

Un disque plein met le téléchargement en pause (« disque plein ») : il repart seul quand la
place revient.

## Propriétés

Double clic sur une ligne, Entrée, ou « Propriétés » :

- fichier, dossier, adresse, page d'origine, statut, taille, téléchargé, débit, temps restant ;
- **Connexions** : le nombre de connexions de ce téléchargement (automatique par défaut) ;
- **Reprise possible** : si le serveur permet de reprendre ;
- **Empreinte** : celle attendue, ou celle trouvée à côté du fichier, et si elle est vérifiée ;
  elle se donne, change ou s'efface ici tant que le téléchargement n'est pas terminé ;
- **Miroirs** : chaque source, son état et son débit ; « Ajouter un miroir » ;
- une **carte des segments** : ce qui est reçu, morceau par morceau ;
- la **limite de débit** de ce téléchargement.

## FTP, FTPS et SFTP

- FTP sans chiffrement n'envoie jamais de mot de passe, sauf si vous l'autorisez pour ce site
  (« FTP : autoriser sans chiffrement », dans les
  [identifiants de sites](08-options.md#identifiants-de-sites)).
- SFTP : au premier contact, AuroDownloader montre l'**empreinte de la clé du serveur**, dans
  les Propriétés (« Clé du serveur SSH », « Accepter la clé… »). Vérifiez-la auprès de
  l'administrateur du serveur avant de l'accepter ; les téléchargements de ce serveur
  reprennent ensuite. Une clé qui change bloque les téléchargements : c'est peut-être une
  attaque.
- Identifiants : nom et mot de passe, ou clé privée SSH, dans les identifiants de sites. Une
  adresse comme `ftp://nom:motdepasse@serveur/fichier` marche aussi : le mot de passe est
  rangé dans le coffre du système et retiré de l'adresse.

## Sites qui demandent un compte

Les identifiants d'un site (Basic ou Digest) se règlent dans les
[identifiants de sites](08-options.md#identifiants-de-sites). Les mots de passe sont gardés
dans le coffre du système (Gestionnaire d'identification de Windows, trousseau de Linux),
jamais dans la base d'AuroDownloader. Pour un site où l'on se connecte par une page, passez par
l'[extension](09-extension.md), qui transmet les cookies.

## Metalink

Un fichier `.meta4` ou `.metalink`, ou son adresse, donne pour chaque fichier ses adresses et
son empreinte. S'il contient plusieurs fichiers, la boîte vous les fait cocher. Chaque morceau
faux est téléchargé de nouveau, de préférence depuis une autre source ; une source qui en
donne deux est écartée.

## Déplacer, renommer, actualiser, retélécharger

- **Déplacer/Renommer…** : un autre nom ou un autre dossier, même sur un autre disque ;
  possible pour un téléchargement qui ne tourne pas, terminé ou non.
- **Actualiser l'adresse…** : remplace un lien expiré. Pour un téléchargement commencé, la
  nouvelle adresse doit donner le même fichier ; ce qui est reçu est gardé.
- **Retélécharger…** : recommence depuis le début. « Remplacer le fichier déjà téléchargé »
  l'écrase ; sinon le nouveau prend un autre nom.

## Où vont les fichiers

Pendant le téléchargement, le fichier porte un nom provisoire en `.part` ; il prend son nom à
la fin. Un nom déjà pris reçoit un suffixe (`fichier (1).iso`). Les
[règles de rangement](07-organisation.md#règles-de-rangement) peuvent choisir le dossier et
le nom.
