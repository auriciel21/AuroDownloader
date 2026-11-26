# Analyse de liens

## Tous les fichiers d'une page

1. « Liens » ouvre « Analyser des liens ».
2. Collez dans « Adresse d'une page web, ou texte contenant des adresses » l'adresse d'une
   page, ou un texte qui contient des adresses, puis « Analyser » ; ou « Importer une liste
   (texte, CSV)… ». La page est lue telle quelle : ses pages liées ne sont parcourues
   qu'avec « Explorer le site » (ci-dessous).
3. AuroDownloader lit la page (sans exécuter de JavaScript) et liste les liens de fichiers,
   avec leur nom, leur taille et leur adresse. Les tailles se lisent auprès des serveurs,
   quatre à la fois.
4. **Filtres** : Extensions (`iso, zip`), Taille minimale et Taille maximale (`10M`,
   `1G`…), Expression régulière sur l'adresse. « Montrer aussi les pages et les liens sans
   extension » montre le reste. Les filtres s'appliquent aussitôt au tableau (Télécharger,
   Nom, Taille, Adresse).
5. Cochez (« Tout cocher », « Tout décocher »), choisissez « Enregistrer dans » et, si besoin,
   un « Sous-dossier », puis « Démarrer » ou « Télécharger plus tard ».

Chaque lien devient un téléchargement ; un lien refusé n'empêche pas les autres.

## Explorer un site

Comme le « Grabber » d'IDM : AuroDownloader lit les pages d'un site de lien en lien et liste
les fichiers qu'elles citent. Rien n'est téléchargé avant votre choix.

1. Dans « Analyser des liens », collez l'adresse d'une page du site, puis, sous
   « Exploration du site », cochez « Explorer le site : lire aussi les pages liées du même
   site ».
2. Réglez l'exploration, ou choisissez un **Modèle** :
   - « Toutes les images du site », « Toutes les vidéos du site », « Tous les fichiers sauf
     pages et images » (archives, documents, musique, programmes, vidéos) : profondeur 5 ;
   - « Audio de la page et d'un niveau en dessous », « Documents de la page et d'un niveau
     en dessous » : profondeur 1.

   Un modèle coche l'exploration et remplit la profondeur et le filtre « Extensions » ;
   changés à la main, le modèle redevient « Personnalisé ».
   - **Profondeur (1 à 5)** : 1 lit la page et les pages qu'elle cite ; 2, aussi celles
     qu'elles citent, et ainsi de suite.
   - **Pages lues au plus** : 200 par défaut, 2 000 au plus.
   - **Pages à ne pas ouvrir (expression régulière)** : par défaut les pages de déconnexion
     (`logout`, `signout`, `déconnexion`…), qui fermeraient une session ; videz le champ pour
     tout ouvrir.
   - **Rester sous le dossier de départ** : seulement les pages sous le dossier de la page
     collée.
   - **Ignorer robots.txt** : lit aussi les pages que le site demande aux robots de ne pas
     lire ; à réserver à vos propres sites.
3. « Analyser » : l'avancement s'affiche (pages lues, en attente, liens trouvés) ;
   « Arrêter l'exploration » s'arrête en gardant ce qui a été trouvé. Fermer la boîte
   l'arrête aussi.
4. À la fin, le tableau gagne deux colonnes : **Niveau** (1 pour un lien de la page de
   départ, 2 pour un lien d'une page qu'elle cite…) et **Trouvé sur** (la page qui le cite).
   Avec « Montrer aussi les pages et les liens sans extension », chaque page dit si elle a été
   lue, ou pourquoi elle ne l'a pas été (au-delà de la profondeur, autre site, robots.txt,
   exclue…).
5. Filtrez et cochez comme pour une page. **Garder l'arborescence du site** range chaque
   fichier dans un sous-dossier au nom du site, puis de ses dossiers (8 niveaux au plus) ;
   sinon, tous vont dans le dossier choisi.

Ce que fait l'exploration :

- seules les pages du **même site** (même nom d'hôte) sont lues ; les fichiers cités par
  elles sont montrés même s'ils sont sur un autre site (images d'un service tiers, par
  exemple) ;
- deux pages à la fois au plus, avec une pause d'un quart de seconde entre deux, pour ne pas
  surcharger le site ; robots.txt est respecté ;
- une page protégée par un mot de passe se lit avec les identifiants enregistrés pour son
  site (Options, Identifiants de sites) ; une page de connexion par formulaire n'est pas remplie ;
- un lien qui mène à un fichier n'est jamais téléchargé pendant l'exploration : il est rangé
  parmi les fichiers ;
- l'exploration s'arrête aussi après 256 Mio de pages lues ou 10 000 liens trouvés.

Depuis la [page distante](11-acces-distant.md), l'exploration se fait sans les identifiants
des sites, et seulement sur Internet (jamais le réseau local).

## Liens copiés

Option « Proposer d'ajouter les liens copiés » (Options, Général, désactivée par défaut) :
quand vous copiez l'adresse d'un fichier ou un lien magnet, une notification du système le
propose ; un clic ouvre « Ajouter ». Le presse-papiers n'est lu que pour cela, et rien n'en est
gardé. Si les notifications d'AuroDownloader sont coupées dans le système, les Options le
disent.
