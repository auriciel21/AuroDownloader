# AuroDownloader : gestionnaire de téléchargement libre

Gestionnaire de téléchargement gratuit et open source pour Windows, Linux et Android.
Un moteur unique, processus autonome, piloté par une API JSON-RPC 2.0. Les interfaces
(bureau, Android, web, CLI, extension navigateur) ne contiennent aucune logique de
téléchargement.

## Référence

- `docs/SPEC.md` : cahier des charges, modèle de données (section 5), protocole du moteur
  (section 6), algorithmes (section 7), tests et feuille de route (section 9). À lire avant
  toute modification du moteur.
- Les deux schémas du document d'origine (architecture, feuille de route) ne sont pas dans
  l'export Markdown, qui les remplace par des marqueurs « embedded content ». L'architecture
  est décrite en texte aux sections 4.2 à 4.6 et 6.2 ; la feuille de route a été recopiée en
  tableau à la section 9.3.
- Si le code et la spécification divergent, ne pas trancher en silence : le signaler, puis
  mettre à jour l'un ou l'autre.

## Structure

- `crates/engine` (`auro-engine`) : le moteur. Phase 1 : `store` (SQLite et migrations,
  fait), `segmenter` (couverture et redécoupage faits ; sources et nombre de connexions à
  écrire), `io` (`.part`, écriture positionnée, points de contrôle, renommage final ;
  préallocation et espace libre à écrire), `net` (préparation, requêtes de segments avec
  `If-Range`, noms de fichier, classement des erreurs ; FTP, proxies à écrire),
  `store::tasks` (tâches, sources, états de la SPEC 5.5, fin de vie), `transfer`
  (exécution d'une tâche : relie `net`, `io` et `store`, redécoupage dynamique ;
  connexions automatiques à écrire). Phase 2 : `rpc` et `scheduler` (vides). En
  phase 1, la CLI appelle le moteur comme une bibliothèque ; l'API JSON-RPC arrive en
  phase 2 avec l'application de bureau (SPEC 9.3).
- `crates/engine/migrations/` : migrations SQL numérotées, intégrées au binaire.
- `crates/cli` (`auro-cli`, binaire `auro`) : ligne de commande.

## Commandes

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p auro-cli -- db migrate auro.db
cargo run -p auro-cli -- probe https://deb.debian.org/debian/README
cargo run -p auro-cli -- get https://deb.debian.org/debian/README -o .
cargo run -p auro-cli -- list
cargo run -p auro-cli -- resume <id-de-tâche>
# Banc de reprise sous arrêt brutal, version longue (la courte tourne avec cargo test) :
AURO_CRASH_CYCLES=1000 AURO_CRASH_SIZE=1073741824 AURO_CRASH_SECONDS=8 \
  cargo test --release -p auro-cli --test crash_resume -- --nocapture
```

## Règles du projet

- La fiabilité passe avant les fonctionnalités : aucune modification de la reprise ou de
  l'intégrité sans test qui la couvre.
- Une migration publiée ne se modifie jamais : ajouter `000N_nom.sql`, l'enregistrer dans
  `MIGRATIONS` (`store.rs`) et mettre à jour la section 5 de la spécification.
- Protocole : ajouter un champ optionnel ou une méthode ne casse rien ; renommer ou supprimer
  exige une nouvelle version majeure (SPEC 6.1). Mettre à jour la section 6.
- Ordre d'écriture : octets écrits et vidés sur disque (`fdatasync`), puis seulement le
  compteur en base (SPEC 5.6, règle 1).
- Aucune télémétrie. Aucun secret en base ni dans les journaux (SPEC 5.1 et 8.2).
- Invariant de couverture (SPEC 5.6, règle 2) : calcul dans `segmenter`, application en
  base dans `store::segments` (vérifié dans la transaction de chaque écriture, contrôle au
  démarrage par `find_coverage_errors`). Ne jamais écrire dans `segments` sans passer par
  ce module.
- Ordre d'écriture, application : `written_bytes` ne s'enregistre que par
  `store::segments::save_progress`, qui n'accepte que des `io::SyncedProgress`, fabriqués
  par `io` après `sync_data`. Ne pas ajouter d'autre chemin d'écriture de ce compteur.
- Noms de fichier venus du serveur : toujours par `net::filename::sanitize` (SPEC 8.1).
  Les messages d'erreur réseau ne citent jamais l'URL ni la valeur d'un en-tête (SPEC 8.2).
- États d'une tâche : toujours par `store::tasks::change_state` ou `complete_task`, qui
  vérifient la table de la SPEC 5.5. Ne jamais écrire `tasks.state` directement.
- Toute modification de `transfer`, `io` ou `store` passe aussi le banc de reprise
  (`crates/cli/tests/crash_resume.rs`) ; un échec s'y rejoue avec `AURO_CRASH_SEED`.
- Redécoupage (SPEC 7.2) : la borne d'un segment en cours ne recule que par
  `ProgressHandle::shrink_end`, avant l'enregistrement de la coupe (`save_split`). Jamais
  l'inverse : la base ne doit pas promettre à un nouveau segment des octets que l'ancien
  écrivain peut encore accepter.
- Prochaine étape : phase 1 (SPEC 9.3), nombre de connexions automatique (SPEC 7.3) dans
  `transfer` : départ à 4, ajout toutes les 2 s si le débit progresse d'au moins 10 %,
  plafond 16 ; division par deux sur 429, 503 ou connexion refusée, plafond par hôte.

## Façon de travailler

- Relire le fichier concerné avant de le modifier, plutôt que de travailler de mémoire.
- Après toute modification : `cargo fmt`, `clippy` et `cargo test`, puis donner le rapport
  complet de compilation et de tests avant de dire que c'est terminé. Ne jamais annoncer
  qu'un changement est prêt sans l'avoir vérifié.
- Vérifier l'équilibre structurel (accolades, parenthèses) de tous les fichiers touchés, pas
  seulement de celui en cours, et les livrer tous.
- Si une fonctionnalité touche plusieurs fichiers, le dire explicitement avant de livrer.
- Quand un comportement visuel ou d'exécution n'est pas vérifiable directement (Android,
  Windows, réseau réel), le dire clairement plutôt qu'affirmer que ça fonctionnera.
