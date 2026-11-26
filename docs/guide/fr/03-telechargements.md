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
- **Enregistrer dans** : le dossier (« Parcourir… » en choisit un autre).
- **Sous-dossier** : créé dans le dossier ci-dessus, par exemple `Musique/Album` ; utile depuis
  l'[interface distante](11-acces-distant.md), qui ne choisit pas de dossier.

« Démarrer » lance aussitôt les téléchargements ; « Télécharger plus tard » les ajoute en
pause.

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

Double clic sur une ligne, Entrée, ou « Propriétés ». La boîte se met à jour pendant le
téléchargement.

- **En tête** : Fichier, Dossier, Adresse, Page d'origine (celle d'où vient le lien, par
  l'extension ou un plugin), Statut, Taille, Téléchargé, Débit, Temps restant, Ajouté le, Fini
  le, puis une barre d'avancement et une **carte** de ce qui est reçu, morceau par morceau.
- **Connexions** : le nombre de connexions de ce téléchargement (automatique par défaut :
  AuroDownloader en ajoute tant que le débit y gagne, et en retire si le serveur sature).
- **Reprise possible** : si le serveur permet de reprendre là où le téléchargement s'est
  arrêté ; sinon, une reprise recommence au début.
- **Empreinte** : celle attendue, ou celle trouvée à côté du fichier, et si elle est vérifiée.
  **Morceaux** (Metalink) : leur nombre et leur taille, et combien ont été retéléchargés
  parce qu'ils étaient faux.
- **Envoyé aux autres pairs** (torrent) : ce qui a été partagé ; le statut donne le ratio.
- **Clé du serveur SSH** (SFTP) : l'empreinte d'une clé encore inconnue, et « Accepter la
  clé… » (voir plus bas).
- **Segments et connexions** : chaque morceau en cours (N°, Téléchargé, État) et la source
  qui l'envoie.
- **Sources** : la Principale et chaque Miroir, avec leur état (À vérifier, Bonne, Lente,
  Écartée) et leur Débit moyen. « Ajouter un miroir », puis
  « Ajouter » : une autre adresse du même fichier, vérifiée avant de servir ; un
  téléchargement en cours repart aussitôt avec elle.
- **Fichiers du torrent** : Télécharger (case), Fichier, Taille, Reçu ; décochez ce que vous
  ne voulez pas, puis « Appliquer le choix ». La liste arrive avec les métadonnées du torrent,
  une fois qu'il a démarré.
- **Débit maximal de ce téléchargement**, puis « Appliquer » : la limite globale s'applique
  aussi, la plus basse l'emporte. Pour un torrent, une nouvelle limite vaut à sa prochaine
  reprise.
- **Empreinte attendue**, puis « Enregistrer » : SHA-256, SHA-1 ou MD5 ; vide pour n'en
  vérifier aucune. Elle se change tant que le téléchargement n'est pas terminé. Après un échec
  d'empreinte, « Relancer » vérifie de nouveau le fichier reçu, sans le retélécharger.
- **En bas** : « Pause » (ou « Reprendre », « Relancer » après un échec, « Arrêter le
  partage » pour un torrent), « Annuler le téléchargement », et, une fois terminé, « Ouvrir »
  et « Afficher dans le dossier ».

## FTP, FTPS et SFTP

- FTP sans chiffrement n'envoie jamais de mot de passe, sauf si vous l'autorisez pour ce site
  (« FTP : autoriser sans chiffrement », dans les
  [identifiants de sites](08-options.md#identifiants-de-sites)).
- SFTP : au premier contact, AuroDownloader montre l'**empreinte de la clé du serveur**, dans
  les Propriétés (« Clé du serveur SSH », « Accepter la clé… », qui demande « Accepter la clé
  de ce serveur ? »). Vérifiez-la auprès de l'administrateur du serveur avant de l'accepter ; les téléchargements de ce serveur
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

- **Déplacer/Renommer…** : « Nom du fichier » et « Dossier » (« Parcourir… »), puis
  « Déplacer » ; même vers un autre disque (le fichier est alors copié, puis l'original
  effacé). Possible pour un téléchargement qui ne tourne pas, terminé ou non. Un nom refusé
  (caractère interdit, nom déjà pris) est dit, jamais corrigé en silence.
- **Actualiser l'adresse…** : remplace un lien expiré ; la boîte montre l'« Adresse
  actuelle », vous donnez la « Nouvelle adresse », puis « Actualiser ». Pour un
  téléchargement commencé, la nouvelle adresse doit donner le même fichier ; ce qui est reçu
  est gardé.
- **Retélécharger…** : recommence depuis le début. « Remplacer le fichier déjà téléchargé »
  l'écrase ; sinon le nouveau prend un autre nom.

## Où vont les fichiers

Pendant le téléchargement, le fichier porte un nom provisoire en `.part` ; il prend son nom à
la fin. Un nom déjà pris reçoit un suffixe (`fichier (1).iso`). Les
[règles de rangement](07-organisation.md#règles-de-rangement) peuvent choisir le dossier et
le nom.
