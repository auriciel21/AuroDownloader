//! Écriture positionnée, préallocation, empreintes ; abstraction du stockage
//! (fichier local ou sélecteur de stockage Android). Voir SPEC §7.5 et §7.9.
//!
//! À faire (phase 1) : fichier `.part` renommé de façon atomique, `fdatasync` avant
//! l'enregistrement des compteurs (SPEC §5.6, règle 1).
