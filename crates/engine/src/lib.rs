//! Moteur de téléchargement d'AuroDownloader.
//!
//! Le moteur est un processus autonome qui exécute les téléchargements et expose une
//! API JSON-RPC ; les interfaces ne contiennent aucune logique de téléchargement.
//! La référence est `docs/SPEC.md` (architecture : section 4).
//!
//! Phase 1 (SPEC §9.3) : `store`, `segmenter`, `net`, `io` et `transfer`. Phase 2 :
//! `scheduler` (files, nouvelles tentatives, reprise au démarrage ; objet `Engine`), puis
//! `rpc`. Torrent, média, post-traitement et plugins arriveront avec leurs phases.

pub mod io;
pub mod net;
pub mod rpc;
pub mod scheduler;
pub mod segmenter;
pub mod store;
#[cfg(any(test, feature = "testing"))]
#[doc(hidden)]
pub mod testing;
pub mod transfer;
