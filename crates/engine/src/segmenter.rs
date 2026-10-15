//! Découpage et redécoupage des segments, répartition sur les sources, nombre de
//! connexions. Voir SPEC §7.2, §7.3 et §7.4.
//!
//! Invariant à tester en priorité (SPEC §5.6, règle 2) : les segments d'une tâche
//! couvrent `[0, total_bytes - 1]` sans trou ni chevauchement, y compris après un
//! nombre aléatoire de redécoupages.
