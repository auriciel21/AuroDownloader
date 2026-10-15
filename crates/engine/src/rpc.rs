//! Serveur JSON-RPC 2.0, transports, authentification, abonnements aux événements.
//! Voir SPEC §6 (protocole du moteur).
//!
//! À faire (phase 2, avec l'application de bureau ; en phase 1 la CLI appelle le moteur
//! comme une bibliothèque, SPEC §9.3) : `engine.hello`, `task.add`, `task.list`, `events.subscribe`,
//! révision `rev` et `sync.snapshot`.
