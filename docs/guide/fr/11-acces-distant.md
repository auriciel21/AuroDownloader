# Accès à distance

L'interface web distante montre la fenêtre d'AuroDownloader dans un navigateur, sur un
téléphone ou un autre ordinateur du même réseau, pour ajouter et suivre des téléchargements.
Elle est désactivée par défaut.

## Activer

Options, Accès à distance :

1. **Piloter les téléchargements depuis un autre appareil** : cochez.
2. **Adresse d'écoute** et **Port** : l'adresse de cet ordinateur sur le réseau local (la liste
   les propose), et le port (7380 par exemple). Windows peut demander d'autoriser
   AuroDownloader dans le pare-feu.
3. Hors de cet ordinateur, la connexion est toujours chiffrée (HTTPS). AuroDownloader fabrique
   son propre certificat ; le navigateur de l'autre appareil avertit la première fois :
   comparez l'**empreinte SHA-256** affichée dans les Options avant d'accepter.
   « Utiliser mon certificat… » prend un certificat à vous à la place.

## Ajouter un appareil

« Ajouter un appareil… » : un **Nom de l'appareil** (« Téléphone »), puis les **Droits** :

- Tout piloter : ajouter, arrêter, supprimer ;
- Ajouter seulement ;
- Regarder seulement.

« Créer le jeton » montre l'**Adresse de la page** et un **QR code** : scannez-le avec
l'appareil, qui ouvre la page connectée. Sinon, ouvrez l'adresse dans son navigateur : la boîte
« Connexion à AuroDownloader » demande le **Jeton**, à copier depuis « Jeton, à coller dans la
page si besoin » (« Copier »), puis « Se connecter ». Le jeton n'est montré qu'une fois ;
« Retirer » un appareil coupe aussitôt son accès. Sur l'appareil, « Se déconnecter » (barre
d'outils de la page) ferme sa session.

## Ce que la page distante ne fait pas

Par sécurité, elle ne choisit pas de dossier sur l'ordinateur (seulement un sous-dossier du
dossier des téléchargements), n'ouvre pas de fichier, ne modifie ni les règles, ni les
plugins, ni les réglages de l'accès à distance, et ne télécharge que depuis Internet, pas
depuis le réseau local de l'ordinateur (sauf `auro remote allow-private on`, par la ligne de
commande).
