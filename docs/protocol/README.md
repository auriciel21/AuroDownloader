# Contrat du protocole

Le protocole JSON-RPC du moteur (SPEC section 6) est décrit ici en schémas JSON
(JSON Schema 2020-12), lisibles par tout outil et par les auteurs de clients. Les tests du
moteur s'y tiennent (SPEC 9.1, « Contrat du protocole », bloquant pour une version).

## Fichiers

| Chemin | Contenu |
|---|---|
| `common.json` | Définitions communes : tâche, file, planification, réglages, erreurs… |
| `methods/<méthode>.json` | Une méthode : `$defs/params` (ce que le client envoie) et `$defs/result` |
| `notifications/<nom>.json` | Une notification du moteur : `$defs/params` |
| `corpus/<client>.json` | Requêtes réelles de chaque client (page, CLI, relais de l'extension, Android, bureau) |
| `v1/` | Les schémas de la version 1 du protocole, figés : ne jamais les modifier |

Chaque schéma a un `$id` sous `https://aurodownloader.org/protocol/`. Cette adresse
nomme le schéma ; elle n'est jamais lue par le réseau.

## Ce que vérifient les tests (`crates/engine/src/rpc/contract.rs`)

- Chaque méthode servie et chaque notification émise a son schéma, et aucun schéma ne
  décrit une méthode qui n'existe pas.
- Chaque réponse (résultat ou erreur) et chaque notification vues par les tests du
  protocole (`rpc::tests`) sont validées contre leur schéma.
- Le corpus de chaque client est rejoué contre un moteur de test (`the_clients_corpus_still_works`).
  Les paramètres de chaque requête suivent leur schéma, et le résultat, ou le code d'erreur,
  est celui attendu.
- Les schémas courants restent compatibles avec ceux de `v1/`
  (`the_protocol_keeps_compatible_with_version_1`). Sont refusés :
  - un champ, une méthode ou une notification retirés ;
  - un paramètre devenu obligatoire, ou une valeur de paramètre plus acceptée ;
  - un type accepté retiré d'un paramètre ;
  - dans une réponse, un champ qui n'est plus toujours présent, ou un type nouveau.

## Faire évoluer le protocole

- **Ajouter** une méthode, un paramètre facultatif ou un champ de réponse ne casse rien
  (SPEC 6.1). Mettre à jour le schéma courant et la SPEC section 6 ; si un client s'en sert,
  ajouter sa requête au corpus. `v1/` ne bouge pas.
- **Un client tolère l'inconnu** : il ignore un champ qu'il ne connaît pas, et traite une
  valeur inconnue (un nouvel état de tâche, un nouveau code d'erreur) comme une valeur
  générique. Une réponse peut donc gagner des valeurs dans une énumération.
- **Retirer ou renommer** exige une nouvelle version majeure du protocole (SPEC 6.1). Le
  moteur continue alors de parler la version 1 aux clients qui la demandent, et la nouvelle
  version se fige à son tour dans `v2/`.

## Corpus

Chaque pas du corpus est `{"method", "params", "expect"?, "save"?, "replay"?}` :
- `expect` liste les issues permises : `"ok"` ou un code d'erreur (par défaut, `["ok"]`) ;
- `save` garde une valeur du résultat sous un nom (`{"task": "/taskId"}`) ;
- `"replay": false` vérifie seulement les paramètres. C'est le cas de `media.installTools`,
  qui téléchargerait.

Remplacés au rejeu :
- `${nom}` seul, par une valeur gardée ou préparée par le test : `dir`, `torrent`,
  `metalink`, `plugin`, `pluginDigest` ;
- dans un texte, `${url:/chemin}` (le serveur de test) et `${dir}`.
