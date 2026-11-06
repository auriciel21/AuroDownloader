# Mesures de performance

Relevés du banc de performance face à aria2 (SPEC 9.1, porte de la phase 1 ; N-01).
Le banc est `crates/cli/tests/perf_vs_aria2.rs` :

```
AURO_ARIA2=chemin/vers/aria2c cargo test --release -p auro-cli --test perf_vs_aria2 -- --ignored --nocapture
```

Chaque mesure lance un processus neuf dans un dossier vide, à nombre de connexions égal
(`auro -c N` face à `aria2c -x N -s N -k 1M --file-allocation=none`). Le temps processeur
est celui du processus, en mode utilisateur et système. « Cœur à 1 Gbit/s » est la part
d'un cœur que coûterait 1 Gbit/s au même coût par octet. Depuis le troisième relevé, la
porte de N-01 est le rapport des coûts par octet auro / aria2. Les 15 % d'un cœur sont un
objectif indicatif, qui n'a de sens que sur la machine de référence (SPEC §3).

## 2026-10-15 : Windows 11, Intel Core i7-3930K, aria2 1.37.0, auro 0.1.0 (d7176a9 + banc)

> **Correction :** les colonnes « Cœur à 1 Gbit/s » de cette section sont fausses. Elles
> venaient de `GetProcessTimes`, qui échantillonne le temps à chaque tic d'horloge et compte
> très mal un processus qui s'endort sans cesse pour ses entrées-sorties. Voir la section
> suivante, mesurée en cycles. Les débits restent valables.

**Serveur local, fichier de 512 Mio, médiane de 5 mesures :**

| Scénario | Outil | Débit (Mio/s) | auro / aria2 | Cœur à 1 Gbit/s |
| --- | --- | --- | --- | --- |
| local, sans bridage | auro -c 4 | 111,3 | 1,60 | 3,3 % |
| local, sans bridage | aria2c -x 4 | 69,5 | | 26,2 % |
| local, 8 Mio/s par connexion | auro -c 16 | 84,5 | 1,28 | 10,2 % |
| local, 8 Mio/s par connexion | auro (automatique) | 41,1 | 0,62 | 2,2 % |
| local, 8 Mio/s par connexion | aria2c -x 16 | 66,2 | | 14,9 % |

**Serveur réel : ISO Debian 13.7 de 756 Mio, `cdimage.debian.org` et ses miroirs,
médiane de 2 mesures :**

| Scénario | Outil | Débit (Mio/s) | auro / aria2 | Cœur à 1 Gbit/s |
| --- | --- | --- | --- | --- |
| serveur réel | auro -c 16 | 31,8 | 1,12 | 8,9 % |
| serveur réel | auro (automatique) | 42,3 | 1,49 | 11,3 % |
| serveur réel | aria2c -x 16 | 28,5 | | 12,1 % |

**Lecture :**

- À nombre de connexions égal, auro va aussi vite qu'aria2 ou plus vite, et coûte
  nettement moins de processeur. Le critère de débit de la porte (écart sous 5 %) est
  tenu dans ces scénarios, et N-01 aussi : moins de 15 % d'un cœur partout.
- Le mode automatique perd face à aria2 réglé sur 16 connexions quand le serveur bride
  chaque connexion et que le téléchargement est court : la règle de la SPEC 7.3 (une
  connexion de plus toutes les 2 s) n'a pas le temps de monter. Face à la configuration
  par défaut d'aria2 (une seule connexion par serveur), il reste devant.
- Sur des mesures de 3 à 4 s, le bruit atteint 10 % : un premier relevé à 256 Mio donnait
  0,90 à 16 connexions, que les mesures plus longues n'ont pas confirmé. Retenir les
  mesures longues et répétées.
- Le débit sans bridage n'atteint pas 1 Gbit/s sur cette machine. La limite est commune
  aux deux outils et vient sans doute du disque ou de l'antivirus, pas des outils. Le
  critère « saturer 1 Gbit/s » reste à vérifier sur une machine plus rapide.
- Le serveur réel répond par des miroirs différents d'une mesure à l'autre : ce sont les
  chiffres les plus bruités.

## 2026-10-15, second relevé : même machine, montée automatique par doublement, temps processeur en cycles

Changements depuis le relevé précédent :
- le mode automatique double ses connexions à chaque fenêtre (SPEC 7.3) ;
- le temps processeur vient de `QueryProcessCycleTime` (cycles exacts, convertis avec la
  fréquence du compteur, étalonnée) ;
- curl (livré avec Windows, une seule connexion) sert de référence : vers le disque, et
  sans rien écrire.

**Serveur local, fichier de 512 Mio, médiane de 5 mesures :**

| Scénario | Outil | Débit (Mio/s) | auro / aria2 | Cœur à 1 Gbit/s |
| --- | --- | --- | --- | --- |
| local, sans bridage | auro -c 4 | 136,5 | 1,76 | 28,3 % |
| local, sans bridage | aria2c -x 4 | 77,4 | | 62,5 % |
| local, sans bridage | curl (disque) | 320,7 | | 36,0 % |
| local, sans bridage | curl (sans disque) | 791,9 | | 10,1 % |
| local, 8 Mio/s par connexion | auro -c 16 | 77,2 | 1,07 | 23,7 % |
| local, 8 Mio/s par connexion | auro (automatique) | 61,1 | 0,85 | 25,3 % |
| local, 8 Mio/s par connexion | aria2c -x 16 | 72,3 | | 55,5 % |

Trois autres campagnes identiques, mesurées avant le passage aux cycles, donnent les mêmes
rapports de débit : mode automatique entre 0,85 et 0,91, auro -c 16 entre 1,03 et 1,27.

**Lecture :**

- **Mode automatique :** il passe de 0,62 à environ 0,87 face à aria2 réglé sur 16
  connexions. Il reste derrière sur un téléchargement de 7 à 8 s, car il passe ses deux
  premières fenêtres de mesure à 4 puis 8 connexions.
- **Coût processeur :** auro coûte 2,2 à 2,4 fois moins qu'aria2. Mais il dépasse N-01
  (moins de 15 % d'un cœur à 1 Gbit/s) sur cette machine.
- **Ce dépassement vient du système, pas d'auro :**
  - recevoir seulement coûte 10 % (curl sans disque) ;
  - recevoir et écrire coûte 36 % (curl vers disque), soit environ 26 points pour le seul
    chemin d'écriture de Windows, antivirus compris ;
  - auro, à 28 %, coûte moins que curl vers disque.
  N-01 tel qu'écrit n'est pas atteignable ici, même par l'outil le plus simple. Il faut le
  redéfinir (par exemple par rapport à curl vers disque sur la même machine) ou le mesurer
  sur une machine de référence.
- **Débit sans bridage :** curl écrit à 321 Mio/s, auro à 136 Mio/s. Auro vide ses données
  sur disque (`fdatasync`) toutes les 2 s et à la fin de chaque segment (règle 1) : son
  débit est très probablement celui du disque (hypothèse, à confirmer en mesurant le disque
  seul). Curl laisse les données dans le cache du système, et le
  fichier n'est pas encore sur disque quand il rend la main. Le coût de la fiabilité est
  là, et il est voulu.

## 2026-10-15, troisième relevé : même machine, HTTP et HTTPS, coût processeur relatif

Changements depuis le relevé précédent :
- N-01 est redéfini (SPEC §3). Sa porte est le rapport des coûts processeur par octet
  auro / aria2, à nombre de connexions égal ; son seuil reste à fixer. Les 15 % d'un cœur
  deviennent un objectif indicatif, sur une machine de référence nommée ;
- nouvelle colonne « Processeur par octet auro / aria2 » ;
- scénarios HTTPS : le serveur de test présente un certificat signé par une autorité créée
  pour le banc. auro la reçoit par `--ca-cert`, curl par `--cacert --ssl-no-revoke`. aria2
  tourne avec `--check-certificate=false`, car sa version Windows ignore
  `--ca-certificate` (son journal : « TLS CA bundle files are not supported ») ;
- machine : celle des relevés précédents, avec l'antivirus actif sur le dossier
  temporaire. Ce n'est pas la machine de référence de N-01.

**Serveur local, fichier de 512 Mio, médiane de 5 mesures :**

| Scénario | Outil | Débit (Mio/s) | Débit auro / aria2 | Processeur par octet auro / aria2 | Cœur à 1 Gbit/s |
| --- | --- | --- | --- | --- | --- |
| local, sans bridage | auro -c 4 | 106,1 | 1,52 | 0,42 | 27,8 % |
| local, sans bridage | aria2c -x 4 | 69,9 | | | 65,8 % |
| local, sans bridage | curl (disque) | 319,6 | | | 36,6 % |
| local, sans bridage | curl (sans disque) | 803,7 | | | 9,9 % |
| local HTTPS, sans bridage | auro -c 4 | 105,3 | 1,56 | 0,63 | 50,9 % |
| local HTTPS, sans bridage | aria2c -x 4 | 67,4 | | | 81,0 % |
| local HTTPS, sans bridage | curl (disque) | 205,4 | | | 57,1 % |
| local HTTPS, sans bridage | curl (sans disque) | 336,9 | | | 32,5 % |
| local, 8 Mio/s par connexion | auro -c 16 | 89,0 | 1,22 | 0,42 | 24,8 % |
| local, 8 Mio/s par connexion | auro (automatique) | 62,8 | 0,86 | 0,41 | 24,4 % |
| local, 8 Mio/s par connexion | aria2c -x 16 | 72,9 | | | 59,5 % |
| local HTTPS, 8 Mio/s par connexion | auro -c 16 | 74,7 | 1,06 | 0,87 | 58,1 % |
| local HTTPS, 8 Mio/s par connexion | auro (automatique) | 58,7 | 0,84 | 0,81 | 54,4 % |
| local HTTPS, 8 Mio/s par connexion | aria2c -x 16 | 70,2 | | | 66,7 % |

**Lecture :**

- **Coût processeur relatif :** auro coûte par octet 0,41 à 0,42 fois aria2 en HTTP, et
  0,63 à 0,87 fois en HTTPS. Il reste moins coûteux qu'aria2 dans tous les scénarios,
  alors qu'aria2 ne vérifie pas le certificat ici.
- **Surcoût de HTTPS :**
  - auro : de 23 à 33 points d'un cœur ;
  - curl : 23 points, avec la pile TLS de Windows (Schannel) ;
  - aria2 : 7 à 15 points, alors que sa version Windows passe aussi par Schannel.
  Le chiffrement d'auro (rustls) coûte donc plus que celui d'aria2. La marge la plus
  faible est en HTTPS bridé à 16 connexions (0,87).
- **Coût fixe par processus (hypothèse, à confirmer) :** sur un essai de 32 Mio, auro
  coûtait en HTTPS deux fois aria2 par octet. Sur 512 Mio, ce n'est plus que 0,63 : une
  grande part du surcoût est fixe. Une mesure isolée donne environ 2,4 ms par poignée de
  main TLS une fois le client chaud, et 13 ms en moyenne sur les vingt premières. Le coût
  fixe viendrait donc surtout du démarrage, par exemple du chargement des racines du
  système, plutôt que de chaque connexion.
- **Proposition de seuil pour N-01 :** 1,0, soit jamais plus coûteux qu'aria2 par octet.
  Le pire cas mesuré (0,87) le tient avec une marge de 13 %, qui reste à confirmer sous
  Linux, où aria2 vérifie le certificat.
- **Débit :** à nombre de connexions égal, auro va aussi vite qu'aria2 ou plus vite en
  HTTPS comme en HTTP. Le mode automatique reste à 0,84–0,86 sur les téléchargements de
  7 à 9 s, comme au relevé précédent.

## 2026-10-16 : Linux (WSL 2), banc équitable, vidages groupés

Machine : la même (Intel Core i7-3930K, 12 processeurs logiques), Ubuntu 20.04.6 sous
WSL 2, système de fichiers ext4 du disque virtuel, sans antivirus. aria2 1.35.0 (celui de
la distribution), curl 7.68.0. C'est une approximation de Linux : le disque virtuel de
WSL rend chaque vidage plus coûteux qu'un disque natif.

Changements de méthode (SPEC §9.1) :
- chaque mesure va jusqu'au fichier sur disque : le banc vide lui-même le fichier après
  chaque outil, car aria2 et curl ne le font pas, alors qu'auro le fait (règle 1) ;
- le temps processeur est celui de toute la machine pendant la mesure, moins celui du
  banc et de son serveur : il compte l'écriture différée par le système ;
- aria2 vérifie ici le certificat HTTPS (`--ca-certificate`), contrairement à Windows.

**Avant correction (f845435), 512 Mio, médiane de 5 :**

| Scénario | Outil | Débit (Mio/s) | Débit auro / aria2 | Processeur par octet auro / aria2 | Cœur à 1 Gbit/s |
| --- | --- | --- | --- | --- | --- |
| local, sans bridage | auro -c 4 | 66,2 | 0,40 | 9,90 | 208,5 % |
| local, sans bridage | aria2c -x 4 | 166,3 | | | 21,1 % |
| local HTTPS, sans bridage | auro -c 4 | 92,9 | 0,63 | 3,43 | 137,1 % |
| local HTTPS, sans bridage | aria2c -x 4 | 146,7 | | | 39,9 % |
| local, 8 Mio/s par connexion | auro -c 16 | 99,1 | 1,21 | 1,37 | 33,4 % |
| local, 8 Mio/s par connexion | auro (automatique) | 32,8 | 0,40 | 10,48 | 256,5 % |
| local, 8 Mio/s par connexion | aria2c -x 16 | 82,0 | | | 24,5 % |
| local HTTPS, 8 Mio/s par connexion | auro -c 16 | 94,9 | 1,16 | 0,82 | 25,8 % |
| local HTTPS, 8 Mio/s par connexion | auro (automatique) | 31,9 | 0,39 | 9,18 | 290,1 % |
| local HTTPS, 8 Mio/s par connexion | aria2c -x 16 | 82,1 | | | 31,6 % |

**Diagnostic :**
- **Où part le coût :** pendant un lancement d'auro sans bridage, `/proc/stat` relève
  6,9 s de temps système et 22,7 s d'attente disque (tous cœurs), contre 0,66 s et
  0,09 s pour aria2. Le relevé `strace` montre 7 `fdatasync` de 1,7 s en moyenne.
- **Ce qui n'est pas en cause :** un script Python qui reproduit le motif d'écriture
  d'auro (4 flux positionnés), avec un vidage toutes les 2 s, va aussi vite que sans
  vidage (environ 200 Mio/s, 2 s de temps système).
- **La cause :** quand chaque flux vide aussi le fichier à sa fin, comme un segment
  d'auro, les vidages simultanés du même fichier font monter le temps système à 6–8 s et
  l'attente disque à 12–20 s.
- **La correction (`io::SyncGate`) :** un seul vidage à la fois par fichier ; les
  demandes faites pendant un vidage attendent le suivant, qui les couvre toutes.

**Après correction, 512 Mio, médiane de 5 :**

| Scénario | Outil | Débit (Mio/s) | Débit auro / aria2 | Processeur par octet auro / aria2 | Cœur à 1 Gbit/s |
| --- | --- | --- | --- | --- | --- |
| local, sans bridage | auro -c 4 | 197,0 | 1,03 | 0,68 | 15,9 % |
| local, sans bridage | aria2c -x 4 | 190,7 | | | 23,5 % |
| local, sans bridage | curl (disque) | 166,3 | | | 18,9 % |
| local, sans bridage | curl (sans disque) | 2619,9 | | | 4,8 % |
| local HTTPS, sans bridage | auro -c 4 | 154,3 | 1,06 | 0,83 | 33,8 % |
| local HTTPS, sans bridage | aria2c -x 4 | 145,2 | | | 40,7 % |
| local HTTPS, sans bridage | curl (disque) | 137,2 | | | 44,7 % |
| local HTTPS, sans bridage | curl (sans disque) | 512,7 | | | 24,4 % |
| local, 8 Mio/s par connexion | auro -c 16 | 98,6 | 1,20 | 0,80 | 18,1 % |
| local, 8 Mio/s par connexion | auro (automatique) | 67,0 | 0,82 | 1,23 | 27,9 % |
| local, 8 Mio/s par connexion | aria2c -x 16 | 81,8 | | | 22,7 % |
| local HTTPS, 8 Mio/s par connexion | auro -c 16 | 92,4 | 1,23 | 0,62 | 35,9 % |
| local HTTPS, 8 Mio/s par connexion | auro (automatique) | 64,5 | 0,86 | 0,80 | 46,7 % |
| local HTTPS, 8 Mio/s par connexion | aria2c -x 16 | 75,4 | | | 58,1 % |

**Lecture :**
- **Portes de la phase 1, à nombre de connexions égal :** toutes tenues sous Linux.
  - Débit : 1,03 à 1,23 fois celui d'aria2 (porte : au moins 0,95).
  - Coût processeur par octet : 0,62 à 0,83 fois (N-01 : au plus 1,0).
- **Objectif indicatif de N-01 :** auro sans bridage coûterait 15,9 % d'un cœur à
  1 Gbit/s, contre 23,5 % pour aria2 et 18,9 % pour curl vers disque. Ce n'est pas la
  machine de référence.
- **Mode automatique :** il reste sous aria2 réglé sur 16 connexions (0,82 à 0,86) sur
  des téléchargements de 7 à 8 s, comme sous Windows. Il ne fait pas partie de la porte.
- **Avant correction :** l'écart venait des vidages simultanés, et non d'un défaut de
  la règle 1. Le même code vide maintenant autant, mais un seul vidage à la fois.

## 2026-10-16, second relevé : Windows 11, banc équitable, vidages groupés

Même machine et mêmes outils que le troisième relevé du 2026-10-15 (aria2 1.37.0 sans
vérification du certificat), commit 1937ef7, antivirus actif. 512 Mio, médiane de 5.

| Scénario | Outil | Débit (Mio/s) | Débit auro / aria2 | Processeur machine par octet auro / aria2 | Processeur du processus (s) |
| --- | --- | --- | --- | --- | --- |
| local, sans bridage | auro -c 4 | 109,2 | 1,53 | 0,58 | 1,31 |
| local, sans bridage | aria2c -x 4 | 71,6 | | | 2,85 |
| local, sans bridage | curl (disque) | 164,7 | | | 1,67 |
| local HTTPS, sans bridage | auro -c 4 | 97,2 | 1,30 | 0,61 | 2,24 |
| local HTTPS, sans bridage | aria2c -x 4 | 74,8 | | | 3,34 |
| local HTTPS, sans bridage | curl (disque) | 130,4 | | | 2,59 |
| local, 8 Mio/s par connexion | auro -c 16 | 87,2 | 1,22 | 1,38 | 1,01 |
| local, 8 Mio/s par connexion | auro (automatique) | 62,9 | 0,88 | 1,52 | 1,02 |
| local, 8 Mio/s par connexion | aria2c -x 16 | 71,3 | | | 1,68 |
| local HTTPS, 8 Mio/s par connexion | auro -c 16 | 76,7 | 1,06 | 0,84 | 2,25 |
| local HTTPS, 8 Mio/s par connexion | auro (automatique) | 56,8 | 0,78 | 0,74 | 2,29 |
| local HTTPS, 8 Mio/s par connexion | aria2c -x 16 | 72,3 | | | 2,89 |

**Lecture :**
- **Débit :** auro dépasse aria2 à nombre de connexions égal, de 1,06 à 1,53.
- **Processeur de la machine : mesure trop bruitée sous Windows.** Au repos, la machine
  utilise déjà 2 à 8 % de ses 12 cœurs (applications de bureau ouvertes), soit 1,4 à
  5,8 s de processeur sur 6 s de mesure : autant que les outils eux-mêmes. Le seul
  rapport au-dessus de 1,0 (1,38, HTTP bridé à 16 connexions) en vient très
  probablement : dans ce scénario, le processus auro coûte 1,01 s contre 1,68 s pour
  aria2. Sous WSL, `/proc/stat` ne voit que la machine virtuelle, qui est au repos.
- **Processeur du processus seul :** auro coûte 0,46 à 0,78 fois aria2 dans tous les
  scénarios à nombre de connexions égal.
- **Méthode :** pour un relevé Windows avec le processeur de la machine, il faut une
  machine au repos (applications fermées), avec et sans exclusion de l'antivirus
  (SPEC §9.1). La porte reste mesurée sous Linux.

## 2026-11-06 : Linux (WSL 2), étape 4 de la phase 2 (limites de débit, espace disque)

Même machine et mêmes outils que le relevé Linux du 2026-10-16 (aria2 1.35.0, curl
7.68.0). 512 Mio, médiane de 5. Le code mesuré ajoute à chaque lecture réseau le
passage par les seaux de débit (vides dans le mode `direct` du banc). Il ajoute aussi
un contrôle de la place libre toutes les 2 s, et l'écriture d'un tampon gardé plus de
500 ms.

**Premier passage, porte vérifiée (`AURO_PERF_ENFORCE=1`) :** une porte manquée.
Dans « HTTPS, 8 Mio/s par connexion », à 16 connexions, le processeur coûte 1,08 fois
celui d'aria2 (porte : 1,0). Dans les autres scénarios à nombre de connexions égal, le
processeur coûte 0,57 à 0,97 fois celui d'aria2 et le débit vaut 0,96 à 1,28 fois le
sien.

**Comparaison A/B, pour savoir si l'étape 4 en est la cause :** trois passages à la
suite, dans les mêmes conditions :
- ec15393, le commit d'avant l'étape 4 ;
- puis le code de l'étape 4 ;
- puis ec15393 de nouveau.

Scénario « HTTPS, 8 Mio/s par connexion », 16 connexions :

| Passage | Code | Processeur machine par octet auro / aria2 | Processeur du processus auro (s) | Processeur du processus aria2 (s) |
| --- | --- | --- | --- | --- |
| porte vérifiée | étape 4 | 1,08 | 1,95 | 1,71 |
| A/B 1 | ec15393 | 0,83 | 1,94 | 1,86 |
| A/B 2 | étape 4 | 1,47 | 1,89 | 1,87 |
| A/B 3 | ec15393 | 1,32 | 1,94 | 1,75 |

**Lecture :**
- **Pas de coût mesurable de l'étape 4.** Le processeur du processus auro reste le même
  avec ou sans elle (1,89 à 1,95 s, contre 1,94 s). Dans les autres scénarios aussi,
  les écarts entre les deux versions restent dans ceux d'une version d'un passage à
  l'autre.
- **La porte N-01 n'est pas reproductible sur ce poste.** L'ancien code passe la porte
  au premier passage (0,83) et la manque au troisième (1,32). La mesure de la porte est
  le processeur de toute la machine virtuelle, qui varie de ±30 % d'un passage à
  l'autre. Celui d'aria2, par exemple, passe de 2,25 s à 1,60 s pour le même
  téléchargement. Le relevé du 2026-10-16 (0,62 dans ce scénario) était donc un passage
  favorable, non une marge acquise.
- **Débit :** en HTTPS sans bridage, auro descend une fois à 0,93 fois aria2 (porte
  0,95) avec le code de l'étape 4, contre 1,20 et 1,06 pour l'ancien code. Son temps
  processeur n'a pas changé (1,25 à 1,30 s, contre 1,31 et 1,32 s). L'ancien code
  varie lui-même de 146 à 119 Mio/s d'un passage à l'autre.
- **Décision (2026-11-06) :** le temps processeur de toute la machine n'est plus un
  critère bloquant. Il varie de ±30 % d'un passage à l'autre, même binaire compris.
  - La porte de N-01 porte désormais sur le temps processeur du processus : rapport par
    octet auro / aria2 au plus égal à 1,0, médiane de chaque grandeur.
  - Le temps de la machine reste relevé, à titre indicatif.
  - Une validation périodique sur une machine Linux native ou en intégration continue
    dédiée est prévue (SPEC §3 et §9.1).
  - Sur les quatre passages de ce relevé, le rapport des temps du processus vaut, à 16
    connexions en HTTPS bridé : 1,14 et 1,01 avec le code de l'étape 4 ; 1,04 et 1,11
    avec l'ancien code. La porte est à refaire avec le banc révisé (médiane de chaque
    grandeur).

**Banc révisé (porte sur le processus, médiane de chaque grandeur), code de l'étape 4,
porte vérifiée :**

| Scénario | Outil | Débit (Mio/s) | Débit auro / aria2 | Processeur du processus par octet auro / aria2 | Processeur machine par octet auro / aria2 (indicatif) | Cœur à 1 Gbit/s (processus) |
| --- | --- | --- | --- | --- | --- | --- |
| local, sans bridage | auro -c 4 | 112,3 | 0,78 ⚠ | 0,87 | 0,70 | 15,1 % |
| local, sans bridage | aria2c -x 4 | 144,4 | | | | 17,3 % |
| local HTTPS, sans bridage | auro -c 4 | 140,8 | 1,20 | 0,91 | 0,90 | 30,8 % |
| local HTTPS, sans bridage | aria2c -x 4 | 117,4 | | | | 33,9 % |
| local, 8 Mio/s par connexion | auro -c 16 | 98,5 | 1,56 | 1,08 ⚠ | 0,98 | 21,7 % |
| local, 8 Mio/s par connexion | auro (automatique) | 63,1 | 1,00 | 1,14 | 1,03 | 22,9 % |
| local, 8 Mio/s par connexion | aria2c -x 16 | 63,3 | | | | 20,0 % |
| local HTTPS, 8 Mio/s par connexion | auro -c 16 | 99,7 | 1,45 | 0,99 | 0,73 | 40,6 % |
| local HTTPS, 8 Mio/s par connexion | auro (automatique) | 66,0 | 0,96 | 0,98 | 0,90 | 40,3 % |
| local HTTPS, 8 Mio/s par connexion | aria2c -x 16 | 69,0 | | | | 41,0 % |

**Lecture :**
- **Deux portes manquées :**
  - débit en HTTP sans bridage (0,78) : auro y va de 112 à 146 Mio/s selon les passages
    du jour, aria2 de 121 à 144 ;
  - processeur du processus en HTTP bridé à 16 connexions (1,08).
- **Sur le temps du processus, auro est à parité avec aria2 à 16 connexions, pas en
  dessous.** Sur les cinq passages du jour, le rapport vaut 0,90 à 1,15 à 16
  connexions, ancien et nouveau code confondus. À 4 connexions sans bridage, il vaut
  0,70 à 0,97.
- **Les marges du 2026-10-16 (0,62 à 0,83) venaient de la mesure de toute la machine.**
  Elle compte pour aria2 le vidage que le banc fait à sa place.
- **La porte de N-01 à 1,0 tombe donc dans le bruit de la mesure à 16 connexions.**
  Pour la tenir avec une marge, il faudrait réduire le coût par connexion d'auro, à
  examiner au profileur.

## 2026-11-06, second relevé : Linux (WSL 2), examen du coût d'auro

Même poste, 512 Mio par téléchargement, code de l'étape 4 (22748d4). Le banc gagne deux
réglages pour profiler (`AURO_PERF_ONLY`, `AURO_PERF_WRAP`) et une colonne indicative :
le temps du processus plus celui que le banc passe à vider ce que l'outil a laissé en
mémoire. Le profileur par échantillons (samply, perf) demande
`perf_event_paranoid` ≤ 1, donc `sudo` : non fait. L'examen passe par `strace` et par le
partage temps utilisateur / temps système.

**HTTP bridé à 16 connexions, un passage sous `strace -c -f` par outil :**

| Appel système | auro -c 16 | aria2c -x 16 |
| --- | --- | --- |
| lectures réseau (`recvfrom`) | 8 222 (64 Kio en moyenne) | 32 951 (16 Kio) |
| écritures disque | 2 233 `pwrite64` (256 Kio) | 32 953 `write` et 32 952 `lseek` |
| `epoll_wait` | 1 042 | 2 072 |
| `fdatasync` | 4 | 0 |

Temps utilisateur / système sur trois passages : auro 0,16 à 0,24 s / 0,65 à 0,82 s ;
aria2 0,22 à 0,39 s / 0,62 à 0,78 s. Vider soi-même 512 Mio coûte 0,04 à 0,09 s de
temps système au processus qui le fait (`dd conv=fdatasync` contre `dd`, cinq passages).

**Banc complet, cinq passages, porte non bloquante :**

| Scénario | Outil | Débit (Mio/s) | Débit auro / aria2 | Processeur du processus par octet auro / aria2 | Processus et vidage laissé au banc, auro / aria2 (indicatif) | Processeur machine par octet auro / aria2 (indicatif) | Cœur à 1 Gbit/s (processus) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| local, sans bridage | auro -c 4 | 124,0 | 0,83 ⚠ | 0,77 | 0,71 | 0,86 | 14,3 % |
| local, sans bridage | aria2c -x 4 | 150,1 | | | | | 18,6 % |
| local HTTPS, sans bridage | auro -c 4 | 105,5 | 0,88 ⚠ | 0,91 | 0,87 | 0,86 | 31,6 % |
| local HTTPS, sans bridage | aria2c -x 4 | 120,0 | | | | | 34,8 % |
| local, 8 Mio/s par connexion | auro -c 16 | 92,3 | 1,26 | 0,90 | 0,84 | 0,73 | 22,1 % |
| local, 8 Mio/s par connexion | auro (automatique) | 67,3 | 0,92 | 0,94 | 0,88 | 0,78 | 23,2 % |
| local, 8 Mio/s par connexion | aria2c -x 16 | 73,3 | | | | | 24,6 % |
| local HTTPS, 8 Mio/s par connexion | auro -c 16 | 94,7 | 1,31 | 0,97 | 0,93 | 1,14 | 40,2 % |
| local HTTPS, 8 Mio/s par connexion | auro (automatique) | 63,9 | 0,89 | 1,03 | 0,99 | 1,33 | 42,8 % |
| local HTTPS, 8 Mio/s par connexion | aria2c -x 16 | 72,1 | | | | | 41,4 % |

**Sans bridage, d'où vient la durée :** chronométré à part, le processus aria2 dure 0,8 à
1,0 s, puis le banc passe environ 2,8 s à vider le fichier (total 3,7 s). Le processus
auro dure 2,8 à 4,6 s, son vidage compris. Sous `strace -T`, auro reçoit les 512 Mio en
1,4 s, avant son premier point de contrôle (2 s), puis vide tout le fichier en une fois
(un `fdatasync` de 2,4 à 2,8 s). Aucune écriture n'est bloquée pendant ce vidage
(`pwrite64` : médiane 0,3 ms, 99e centile 1,7 ms).

**Essai : points de contrôle toutes les 250 ms au lieu de 2 s** (copie modifiée, hors
dépôt), scénarios sans bridage, versions alternées A B A B, cinq passages chacune :

| Version | HTTP : auro (Mio/s), auro / aria2 | HTTPS : auro (Mio/s), auro / aria2 | Processeur du processus, HTTP / HTTPS |
| --- | --- | --- | --- |
| A, 2 s | 131,3 ; 1,02 | 104,5 ; 0,81 | 0,82 / 0,89 |
| B, 250 ms | 136,3 ; 0,89 | 160,4 ; 1,34 | 0,81 / 0,89 |
| A, 2 s | 152,9 ; 1,22 | 100,0 ; 0,80 | 0,73 / 0,91 |
| B, 250 ms | 205,1 ; 1,61 | 181,3 ; 1,47 | 0,76 / 0,93 |

**Lecture :**
- **Aucun surcoût d'auro par connexion n'apparaît.** auro fait quatre fois moins d'appels
  système qu'aria2 et passe moins de temps en mode utilisateur. Les deux outils passent
  75 à 80 % de leur temps dans le noyau (copies réseau et disque). La différence restante
  est le vidage qu'auro paie lui-même (règle 1) : 0,84 et 0,93 à 16 connexions, une fois
  ce vidage compté pour les deux.
- **Porte N-01 tenue dans ce relevé** (0,77 à 0,97 à nombre de connexions égal), après
  0,90 à 1,15 sur les passages précédents du jour. À 16 connexions, la mesure reste à
  ±10 % d'un passage à l'autre.
- **Sans bridage, le débit mesure surtout le disque virtuel** (environ 180 Mio/s), pas
  le réseau ni le processeur. auro reçoit plus vite que le disque n'écrit, mais ne
  commence à vider qu'au premier point de contrôle.
- **Commencer à vider plus tôt recouvre le disque et le réseau.** Avec des points de
  contrôle toutes les 250 ms, auro passe de 100 à 105 Mio/s à 160 à 181 Mio/s en HTTPS,
  sans coût processeur mesurable. En HTTP, le bruit est plus fort (0,89 à 1,61). Un
  intervalle fixe de 250 ms viderait cependant quatre fois par seconde même un
  téléchargement lent. Proposition soumise : un point de contrôle aussi après un volume
  reçu (par exemple 32 Mio), en plus des 2 s (SPEC §7.5).

## 2026-11-06, troisième relevé : Linux (WSL 2), point de contrôle par volume

Même poste. Un point de contrôle a lieu aussi dès que les connexions ont remis 32 Mio au
système depuis le précédent (SPEC §7.5). 512 Mio, cinq passages, porte non bloquante.

**Banc complet :**

| Scénario | Outil | Débit (Mio/s) | Débit auro / aria2 | Processeur du processus par octet auro / aria2 | Processus et vidage laissé au banc, auro / aria2 (indicatif) | Processeur machine par octet auro / aria2 (indicatif) | Cœur à 1 Gbit/s (processus) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| local, sans bridage | auro -c 4 | 119,0 | 1,31 | 0,91 | 0,81 | 0,83 | 16,1 % |
| local, sans bridage | aria2c -x 4 | 90,9 | | | | | 17,8 % |
| local HTTPS, sans bridage | auro -c 4 | 107,4 | 1,26 | 0,86 | 0,82 | 0,82 | 30,8 % |
| local HTTPS, sans bridage | aria2c -x 4 | 85,5 | | | | | 35,8 % |
| local, 8 Mio/s par connexion | auro -c 16 | 114,6 | 1,59 | 0,97 | 0,89 | 0,91 | 21,1 % |
| local, 8 Mio/s par connexion | auro (automatique) | 67,1 | 0,93 | 0,94 | 0,86 | 1,05 | 20,4 % |
| local, 8 Mio/s par connexion | aria2c -x 16 | 72,0 | | | | | 21,7 % |
| local HTTPS, 8 Mio/s par connexion | auro -c 16 | 111,6 | 1,50 | 1,06 ⚠ | 1,02 | 0,85 | 43,4 % |
| local HTTPS, 8 Mio/s par connexion | auro (automatique) | 72,3 | 0,97 | 1,02 | 0,99 | 0,87 | 41,9 % |
| local HTTPS, 8 Mio/s par connexion | aria2c -x 16 | 74,3 | | | | | 41,1 % |

**A/B à 16 connexions, bridé** : A avec le seuil, B sans (même code, seuil infini),
ordre A B A B, cinq passages chacun. Temps du processus auro en secondes, rapport à aria2
entre parenthèses :

| Scénario | A | B | A | B |
| --- | --- | --- | --- | --- |
| HTTP : débit auro (Mio/s) | 107,2 | 98,3 | 114,4 | 99,4 |
| HTTP : processeur | 0,92 (1,01) | 0,92 (1,05) | 0,95 (1,00) | 0,88 (0,93) |
| HTTPS : débit auro (Mio/s) | 111,8 | 97,4 | 111,3 | 97,7 |
| HTTPS : processeur | 1,82 (1,07) | 1,84 (1,02) | 1,73 (1,05) | 1,74 (0,99) |

**Bancs de reprise :** `crash_resume`, 300 cycles sur 64 Mio : un premier passage
échoue sur un arrêt tombé entre le renommage du `.part` et l'enregistrement de la fin,
état que le moteur termine à la reprise (SPEC §7.4, étape 7) mais que le contrôle commun
(`testing::Checker`) refusait. Le contrôle l'accepte désormais, si tous les segments sont
finis et le fichier final complet et exact, et le compte. Second passage : 204 arrêts
brutaux, 171 reprises avec des octets enregistrés, 64 arrêts sur un découpage redécoupé,
96 fichiers vérifiés (graine 1793969166062811701). `crash_daemon`, 50 cycles : 40 arrêts
brutaux, 10 arrêts demandés, 26 fichiers vérifiés. Disque vraiment plein (tmpfs de
8 Mio) : 5 963 776 octets enregistrés, exacts, puis la tâche finit.

**Lecture :**
- **Sans bridage, auro dépasse aria2** (1,26 à 1,31), parce qu'il écrit sur disque
  pendant la réception. aria2 était lent ce jour-là (86 à 91 Mio/s, contre 120 à 150
  dans les relevés précédents) : le rapport est flatteur, mais le gain d'auro est réel,
  de 100 à 105 Mio/s à 107 à 119 Mio/s en HTTPS.
- **À 16 connexions bridées, le débit gagne 10 à 15 %** (107 à 114 Mio/s contre 97 à
  99), sans coût processeur mesurable : le temps du processus est le même avec ou sans
  le seuil.
- **La porte N-01 à 16 connexions reste à parité** : 0,93 à 1,07 selon les passages,
  avec ou sans seuil, soit 0,86 à 1,03 en comptant pour les deux le vidage sur disque.
  L'écart restant correspond au vidage qu'auro paie lui-même (règle 1). La tenir à 1,0
  sur chaque passage n'est pas affaire d'optimisation par connexion (relevé précédent).

## 2026-11-06, quatrième relevé : Linux (WSL 2), nouvelle porte de N-01

Même poste. La porte de N-01 porte désormais sur le temps du processus plus celui du
vidage sur disque laissé au banc, par octet, auro / aria2, seuil 1,0 (SPEC §3 et §9.1).
Le processus seul devient indicatif. 512 Mio, cinq passages, porte bloquante
(`AURO_PERF_ENFORCE=1`).

| Scénario | Outil | Débit (Mio/s) | Débit auro / aria2 | Processeur par octet, processus et vidage laissé au banc, auro / aria2 | Processus seul, auro / aria2 (indicatif) | Processeur machine par octet auro / aria2 (indicatif) | Cœur à 1 Gbit/s (processus) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| local, sans bridage | auro -c 4 | 108,1 | 1,12 | 0,76 | 0,82 | 0,81 | 14,9 % |
| local, sans bridage | aria2c -x 4 | 96,6 | | | | | 18,2 % |
| local HTTPS, sans bridage | auro -c 4 | 122,4 | 1,22 | 0,93 | 0,97 | 0,97 | 36,5 % |
| local HTTPS, sans bridage | aria2c -x 4 | 100,0 | | | | | 37,5 % |
| local, 8 Mio/s par connexion | auro -c 16 | 105,4 | 1,49 | 0,92 | 0,99 | 0,94 | 21,7 % |
| local, 8 Mio/s par connexion | auro (automatique) | 72,8 | 1,03 | 0,91 | 0,98 | 1,22 | 21,6 % |
| local, 8 Mio/s par connexion | aria2c -x 16 | 70,6 | | | | | 22,0 % |
| local HTTPS, 8 Mio/s par connexion | auro -c 16 | 110,3 | 1,38 | 1,03 ⚠ | 1,07 | 1,35 | 41,4 % |
| local HTTPS, 8 Mio/s par connexion | auro (automatique) | 72,1 | 0,90 | 1,01 | 1,05 | 1,12 | 40,5 % |
| local HTTPS, 8 Mio/s par connexion | aria2c -x 16 | 80,0 | | | | | 38,6 % |

Le banc échoue : porte manquée en HTTPS bridé à 16 connexions (1,03). Les portes ne
s'appliquent pas au mode automatique.

**Lecture :**
- **En HTTP, la nouvelle porte passe nettement** : 0,76 sans bridage, 0,92 à
  16 connexions. Sur les relevés précédents du même code, à 16 connexions : 0,89 à 0,94.
- **En HTTPS à 16 connexions, elle ne passe pas, de peu mais à chaque fois** : 1,02
  (troisième relevé), 1,03 et 1,01 (passages A de l'A/B), 1,03 ici. Ce n'est plus du
  bruit autour de 1,0 : auro y coûte 1 à 3 % de plus qu'aria2, vidage compris. En
  HTTPS, le vidage pèse peu (0,06 s sur 1,7 s pour aria2) ; l'écart vient du chiffrement
  et de la lecture, pas de l'écriture.
- Sans bridage en HTTPS (quatre connexions, lectures pleines), auro reste en dessous
  (0,82 à 0,93). L'écart n'apparaît qu'avec seize connexions bridées, où chaque
  connexion reçoit peu à la fois : piste à examiner (taille des lectures TLS, nombre de
  réveils par connexion).

## 2026-11-06, cinquième relevé : Linux (WSL 2), d'où vient l'écart en HTTPS

Même poste : processeur Intel Core i7-3930K (2011 ; AES-NI et PCLMUL, ni AVX2 ni MOVBE).
Examen du cas où la porte manquait (HTTPS bridé à 16 connexions, 1,01 à 1,03).

**Écarté :**
- Suite TLS : la même pour les deux outils, TLS 1.3 AES-256-GCM (38 connexions relevées
  sur une copie d'essai du serveur de test).
- Bibliothèque de chiffrement : déchiffrer 512 Mio en enregistrements de 16 Kio coûte
  0,48 à 0,55 s avec GnuTLS 3.6.13 (aria2) et 0,50 à 0,60 s avec aws-lc (auro), copie
  comprise, en passages alternés.
- Appels système : auro en fait moins (32 900 lectures de 16 406 octets, un enregistrement
  TLS chacune, contre 71 200 lectures pour aria2 ; 1 230 `pwrite64` contre 32 770 `write`
  et autant de `lseek`).

**Profil échantillonné** (samply, 4 kHz, temps processeur du processus, 512 Mio) :

| Poste | auro | aria2 |
| --- | --- | --- |
| Déchiffrement (AES-CTR et GHASH) | 0,63 s | 0,63 s |
| Écriture vers le système (noyau compris) | 0,61 s, plus 0,13 s de vidage | 0,53 s, plus 0,03 s de `lseek` |
| Réception (noyau compris) | 0,20 s | 0,22 s |
| Copies mémoire | 0,21 s | 0,15 s |
| Amorçage de l'aléa d'aws-lc | 0,07 s | |
| Ouvertures et fermetures de fichiers | | 0,13 s |
| rustls, hyper, tokio, code d'auro (exclusif) | quelques ms chacun | |
| **Total échantillonné** | **2,19 s** | **2,15 s** |

**Deux coûts propres à auro :**
- **Amorçage de l'aléa d'aws-lc : 56 à 60 ms de processeur, une fois par processus**
  (trois mesures isolées), au premier tirage aléatoire, donc à la première connexion
  TLS. aws-lc y collecte de l'entropie par la gigue du processeur (`jent_*`, compilé
  sans optimisation). Compilé avec `AWS_LC_SYS_NO_JITTER_ENTROPY=1`, le premier tirage
  ne coûte plus rien (0,0 ms) : aws-lc s'amorce alors sur l'entropie du système. Ce coût
  n'existe qu'en HTTPS ; dans `aurod`, il ne se paie qu'au démarrage du moteur, mais le
  banc lance un processus par téléchargement.
- **Une copie de plus qu'aria2 :** chaque morceau reçu est recopié dans le tampon
  d'écriture de 256 Kio (`SegmentWriter::write`, 0,10 s pour 512 Mio).

**A/B** (A : code actuel ; B : même code compilé sans l'entropie de gigue), ordre
A B A B, cinq passages chacun, porte de N-01 (processus et vidage, auro / aria2) :

| Scénario | A | B | A | B |
| --- | --- | --- | --- | --- |
| HTTPS sans bridage, 4 connexions | 1,01 | 0,85 | 0,88 | 0,81 |
| HTTPS bridé, 16 connexions | 0,93 | 0,90 | 0,91 | 0,93 |

**Lecture :**
- Ce jour-là, le code actuel passe la porte à 16 connexions (0,91 et 0,93) : aria2 y a
  coûté 1,84 à 1,92 s, contre 1,66 à 1,72 s lors des relevés précédents, pour un auro
  inchangé (1,77 à 1,81 s). Le rapport suit donc encore les variations d'aria2, de
  l'ordre de ±5 %.
- L'effet de l'entropie (environ 3 % du temps d'auro en HTTPS) se voit à la mesure
  isolée, pas dans ce bruit.
