# Mesures de performance

Relevés du banc de performance face à aria2 (SPEC 9.1, porte de la phase 1 ; N-01).
Le banc est `crates/cli/tests/perf_vs_aria2.rs` :

```
AURO_ARIA2=chemin/vers/aria2c cargo test --release -p auro-cli --test perf_vs_aria2 -- --ignored --nocapture
```

Chaque mesure lance un processus neuf dans un dossier vide, à nombre de connexions égal
(`auro -c N` face à `aria2c -x N -s N -k 1M --file-allocation=none`). Le temps processeur
est celui du processus, en mode utilisateur et système. « Cœur à 1 Gbit/s » est la part
d'un cœur que coûterait 1 Gbit/s au même coût par octet ; N-01 demande moins de 15 %.

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
