//! Moteur de téléchargement d'AuroDownloader.
//!
//! Le moteur est un processus autonome qui exécute les téléchargements et expose une
//! API JSON-RPC ; les interfaces ne contiennent aucune logique de téléchargement.
//! La référence est `docs/SPEC.md` (architecture : section 4).
//!
//! Phase 1 (SPEC §9.3) : `store`, `segmenter`, `net`, `io` et `transfer`. Phase 2 :
//! `scheduler` (files, nouvelles tentatives, reprise au démarrage ; objet `Engine`),
//! `rpc`, `limit` (limites de débit), `launch` (démarrage d'`aurod` par ses clients).
//! Phase 3 : `secrets` (coffre du système, pour les cookies et jetons des tâches).
//! Torrent, média, post-traitement et plugins arriveront avec leurs phases.

pub mod claims;
pub mod io;
pub mod launch;
pub mod limit;
pub mod linkscan;
pub mod media;
pub mod metalink;
pub mod mirrors;
pub mod moves;
pub mod net;
pub mod paths;
pub mod power;
pub mod remote;
pub mod rpc;
pub mod schedule;
pub mod scheduler;
pub mod secrets;
pub mod segmenter;
pub mod store;
#[cfg(any(test, feature = "testing"))]
#[doc(hidden)]
pub mod testing;
pub mod torrent;
pub mod transfer;
