-- Règles de rangement et commande de fin (F-32, F-34, SPEC 7.21, phase 7, étape 4,
-- depuis le 2026-11-16).
--
-- La table `rules` existe depuis la migration 1 : `match_json` (critères) et
-- `actions_json` (dossier, modèle de nom, commande), au format du protocole. Elle reçoit
-- un nom.
ALTER TABLE rules ADD COLUMN name TEXT NOT NULL DEFAULT '';

-- Tâche ajoutée sans dossier choisi : le dossier des téléchargements d'alors, base des
-- dossiers relatifs des règles, qui peuvent la ranger (de nouveau à un retéléchargement).
-- NULL : dossier choisi, ou changé par « Déplacer/Renommer ».
ALTER TABLE tasks ADD COLUMN rule_base TEXT;

-- Règle retenue à la préparation ; sa commande de fin s'exécute à la fin de la tâche.
ALTER TABLE tasks ADD COLUMN rule_id TEXT;
