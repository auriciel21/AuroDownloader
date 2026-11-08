-- « Déplacer/Renommer » (SPEC §7.12, depuis le 2026-11-08).

-- Nom choisi par l'utilisateur : la préparation le prend au lieu de celui du serveur
-- (un nom libre à côté s'il est déjà pris), et il survit au retéléchargement. NULL : le
-- nom du serveur.
ALTER TABLE tasks ADD COLUMN requested_name TEXT;

-- Déplacement en cours du fichier d'une tâche (le fichier final d'une tâche terminée, le
-- `.part` des autres). La ligne est écrite avant de toucher au disque, et effacée quand le
-- disque et la base sont d'accord ; au démarrage, le moteur termine ou annule celles
-- qu'un arrêt brutal a laissées.
--   phase 'moving' : la tâche porte encore l'ancien dossier et l'ancien nom ;
--   phase 'moved'  : elle porte les nouveaux, il ne reste qu'à effacer l'original
--                    (déplacement vers un autre volume, par copie).
CREATE TABLE file_moves (
  task_id    TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
  from_path  TEXT NOT NULL,
  to_path    TEXT NOT NULL,
  temp_path  TEXT,              -- copie vers un autre volume, avant son renommage en to_path
  dest_dir   TEXT NOT NULL,     -- dossier et nom de la tâche une fois le fichier déplacé
  file_name  TEXT NOT NULL,
  phase      TEXT NOT NULL CHECK (phase IN ('moving', 'moved')),
  started_at INTEGER NOT NULL
);
