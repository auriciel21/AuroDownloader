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
- **Ce qu'il reste à décider :** la méthode de la porte (plus de mesures, rapport par
  passage alterné, machine Linux native ou intégration continue). Avant cela, la porte
  N-01 ne peut pas départager deux versions sur ce poste.
