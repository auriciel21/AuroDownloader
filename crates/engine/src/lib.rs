//! Moteur de téléchargement d'AuroDownloader.
//!
//! Le moteur est un processus autonome qui exécute les téléchargements et expose une
//! API JSON-RPC ; les interfaces ne contiennent aucune logique de téléchargement.
//! La référence est `docs/SPEC.md` (architecture : section 4).
//!
//! Phase 1 (SPEC §9.3) : `store`, `segmenter`, `net` et `io` ; la CLI appelle le moteur
//! comme une bibliothèque. `rpc` et `scheduler` arrivent en phase 2 ; torrent, média,
//! post-traitement et plugins arriveront avec leurs phases.

pub mod io;
pub mod net;
pub mod rpc;
pub mod scheduler;
pub mod segmenter;
pub mod store;
