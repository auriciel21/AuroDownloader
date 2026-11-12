# Prototype des plugins : wasmtime face à wasmi

Phase 6, étape 4a (F-41), décisions de l'utilisateur du 2026-11-12 : comparer deux moteurs
WebAssembly sur le même plugin d'essai avant d'écrire le module `plugins` du moteur. Ne fait
pas partie du produit ; espace de travail Cargo à part.

## Ce qui est essayé

- `sample` : plugin d'essai en Rust (`wasm32-unknown-unknown`, ni WASI ni accès au
  système ; 114 Kio avec `serde_json`). Hébergeur fictif `fichiers.exemple` : il demande la
  page à l'hôte, y trouve les liens de téléchargement et les rend (adresse, nom, taille).
  Exports hostiles : boucle sans fin, mémoire par blocs de 16 Mio jusqu'au refus, récursion
  de profondeur donnée, page d'un domaine non déclaré.
- `host` : charge le plugin avec `--features wasmtime` (wasmtime 50.0.1, Cranelift) ou
  `--features wasmi` (wasmi 2.0.0, interpréteur) ; sans l'un ni l'autre, binaire de
  référence (même code, sans moteur). Une instance neuve par appel. Limites : 10⁹ unités
  de calcul (« fuel »), 64 Mio de mémoire, pile de 512 Kio (wasmtime) ; pour wasmi,
  10 000 appels imbriqués et 1 Mio de pile de valeurs (sa limite par défaut, 1 000 appels,
  arrêtait déjà une récursion de 1 000 niveaux).
- Interface (identique pour les deux) : WebAssembly de base, messages JSON dans la mémoire
  du plugin ; exports `auro_alloc`, `auro_resolve` ; imports `auro.fetch` (l'hôte vérifie
  le domaine, puis écrit la réponse dans une zone demandée au plugin) et `auro.log`. Le
  réseau est simulé : l'hôte sert une page de 1 Mio (605 liens à trouver parmi des
  milliers).

```
cargo build --release -p sample --target wasm32-unknown-unknown
cargo build --release -p host --features wasmi     # ou wasmtime, ou rien
target/release/host target/wasm32-unknown-unknown/release/sample.wasm
```

## Mesures (2026-11-12)

Windows 11 (poste de travail) et Linux (WSL Ubuntu, système de fichiers Linux), en
`--release`, 3 passages ; « appel » : instance neuve, page de 1 Mio, médiane de 20.

| | wasmtime 50 | wasmi 2.0 |
| --- | --- | --- |
| Taille ajoutée au binaire, Windows | + 11,0 Mo (11,3 Mo contre 0,23) | + 2,3 Mo (2,6 Mo) |
| Taille ajoutée, Linux | + 15,6 Mo (16,2 Mo contre 0,60) | + 3,7 Mo (4,3 Mo) |
| Binaire Android (aarch64, NDK 27, API 26) | 13,0 Mo, compilé en 5 min 37 s | 4,4 Mo, compilé en 1 min 28 s |
| Crates ajoutées (dépendances normales) | 96 | 15 |
| Compilation du plugin au chargement | 213 à 334 ms | 1,1 à 1,7 ms |
| Appel sur la page de 1 Mio, Windows | 13,1 à 13,8 ms | 85,9 à 86,9 ms |
| Appel, Linux | 13,2 à 17,6 ms | 84,9 à 133 ms |
| Unités de calcul de cet appel | 56 à 60 millions | 125 à 147 millions |
| 10⁹ unités épuisées (boucle sans fin) en | 73 à 99 ms | 328 à 549 ms |
| Mémoire de pointe du processus, Windows / Linux | 60 / 64 Mio | 76 / 58 Mio |
| Référence sans moteur | 8,9 / 6,2 Mio | |

Épreuves du bac à sable, identiques sous Windows et Linux, pour les deux moteurs :

- boucle sans fin : arrêtée, `OutOfFuel` ;
- mémoire démesurée : la croissance au-delà de 64 Mio est refusée, le plugin s'arrête
  (`UnreachableCodeReached`, l'allocateur de Rust abandonne) ;
- récursion de 1 000 et 5 000 niveaux : passent ; de 10⁸ : arrêtée, `StackOverflow`
  (wasmtime, pile native) ou `MemoryOutOfBounds` (wasmi : la pile propre au plugin, dans
  sa mémoire, déborde d'abord ; Rust place cette pile au début de la mémoire, son
  débordement sort de la mémoire au lieu d'écraser les données) ;
- domaine non déclaré : refusé par l'hôte, le plugin le voit ;
- après les épreuves, le moteur sert encore une résolution correcte.

Android : les deux se compilent ; rien n'a été exécuté sur un appareil (aucun branché le
2026-11-12). Licences : `cargo deny check licenses` passe (aucune licence nouvelle ;
wasmtime : Apache-2.0 WITH LLVM-exception, déjà admise).

## Lecture

- Vitesse : wasmtime est 6 fois plus rapide une fois compilé ; mais un plugin d'hébergeur
  fait un appel par lien ajouté, sur une page en général bien plus petite que 1 Mio, et
  attend le réseau (une page demandée prend couramment plus de 100 ms) : les 86 ms de wasmi
  sur 1 Mio restent du même ordre que l'attente du réseau.
- Démarrage : wasmtime compile chaque plugin en code machine au chargement (environ
  250 ms). Garder le code compilé (`Module::serialize`) l'éviterait, mais le moteur
  chargerait alors du code machine depuis un fichier, qu'il faudrait protéger comme un
  exécutable.
- Taille et dépendances : wasmtime ajoute 4 à 5 fois plus au binaire et 6 fois plus de
  crates (surface à suivre, compilation plus longue, surtout pour Android).
- Sûreté : wasmi interprète, sans produire de code machine ni de mémoire exécutable ;
  une erreur de son code reste une erreur d'interpréteur. wasmtime produit du code machine
  (Cranelift) ; une erreur du compilateur peut donner du code qui sort du bac à sable. Les
  deux sont maintenus activement et soumis au fuzzing par leurs auteurs.
- Limites : mêmes mécanismes (fuel, mémoire, pile), mais les unités de calcul ne se
  comparent pas d'un moteur à l'autre ; le budget se fixe par moteur. La pile par défaut
  de wasmi (1 000 appels) est trop courte : à régler.
- Interface : WebAssembly de base et messages JSON conviennent aux deux ; un module
  `plugins` écrit ainsi pourrait changer de moteur plus tard.

Recommandation : wasmi, pour sa taille, son démarrage, son absence de code machine et sa
simplicité de compilation (Android plus tard) ; vitesse suffisante pour des hébergeurs.

Décision de l'utilisateur du 2026-11-12 : wasmi.
