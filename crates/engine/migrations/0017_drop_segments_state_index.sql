-- Index inutile (SPEC 5.4, audit S9, depuis le 2026-11-24).
--
-- Aucune requête ne choisit les segments par leur état : toutes passent par la tâche, que
-- la clé primaire (task_id, idx) couvre. L'index coûtait une écriture à chaque changement
-- d'état d'un segment.
DROP INDEX IF EXISTS idx_segments_state;
