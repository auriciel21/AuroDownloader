-- Vidéo (SPEC 7.17, étape 4c, depuis le 2026-11-09).
--
-- Un flux direct d'une vidéo se télécharge comme une tâche HTTP interne, rattachée à la
-- tâche vidéo par `parent_id` : le planificateur ne la lance jamais lui-même, les listes
-- et l'historique ne la montrent pas ; elle disparaît avec sa tâche vidéo.
ALTER TABLE tasks ADD COLUMN parent_id TEXT REFERENCES tasks(id) ON DELETE CASCADE;
CREATE INDEX tasks_parent ON tasks(parent_id) WHERE parent_id IS NOT NULL;

-- Flux retenus d'une tâche vidéo, dans l'ordre de la fusion. `child_id` : sa tâche HTTP
-- interne (flux direct) ; NULL pour un flux fragmenté, que yt-dlp télécharge. `done` :
-- fichier du flux complet et vidé sur disque, sous `file_name`.
CREATE TABLE media_streams (
  task_id   TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  position  INTEGER NOT NULL,
  format_id TEXT NOT NULL,
  protocol  TEXT NOT NULL,
  ext       TEXT NOT NULL,
  has_video INTEGER NOT NULL CHECK (has_video IN (0, 1)),
  has_audio INTEGER NOT NULL CHECK (has_audio IN (0, 1)),
  file_name TEXT NOT NULL,
  size      INTEGER,
  child_id  TEXT REFERENCES tasks(id) ON DELETE SET NULL,
  done      INTEGER NOT NULL DEFAULT 0 CHECK (done IN (0, 1)),
  PRIMARY KEY (task_id, position)
);
