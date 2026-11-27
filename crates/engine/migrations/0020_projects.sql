-- Projets enregistrés et programmés (SPEC §7.18 quater, F-44, étape C, décisions de
-- l'utilisateur du 2026-11-27).

-- Une copie : mode (copy : pages, ressources et fichiers ; files : fichiers seuls, les pages
-- lues sans être gardées), arborescence des fichiers en mode files, filtres d'un projet
-- (filter_ext non NULL : les fichiers sont pris pendant l'exploration ; extensions en
-- minuscules séparées par des virgules, vide : toutes), numéro du lancement (run).
ALTER TABLE site_jobs ADD COLUMN mode TEXT NOT NULL DEFAULT 'copy'
  CHECK (mode IN ('copy', 'files'));
ALTER TABLE site_jobs ADD COLUMN keep_tree INTEGER NOT NULL DEFAULT 1
  CHECK (keep_tree IN (0, 1));
ALTER TABLE site_jobs ADD COLUMN filter_ext TEXT;
ALTER TABLE site_jobs ADD COLUMN filter_regex TEXT;
ALTER TABLE site_jobs ADD COLUMN filter_min INTEGER CHECK (filter_min >= 0);
ALTER TABLE site_jobs ADD COLUMN filter_max INTEGER CHECK (filter_max >= 0);
ALTER TABLE site_jobs ADD COLUMN run INTEGER NOT NULL DEFAULT 1 CHECK (run >= 1);

-- Un élément : validateurs de sa dernière copie (requêtes conditionnelles d'une mise à
-- jour), dernier lancement qui l'a vu (run) et dernier qui a écrit ses octets
-- (fetched_run ; NULL : jamais copié). Un élément vu sans être réécrit est inchangé.
ALTER TABLE site_items ADD COLUMN etag TEXT;
ALTER TABLE site_items ADD COLUMN last_modified TEXT;
ALTER TABLE site_items ADD COLUMN run INTEGER NOT NULL DEFAULT 1;
ALTER TABLE site_items ADD COLUMN fetched_run INTEGER;
UPDATE site_items SET fetched_run = 1 WHERE state IN ('fetched', 'done');

-- Un projet : réglages de l'exploration, filtres, copie ou fichiers seuls, dossier (NULL :
-- celui des téléchargements du moteur), file, programme (schedule_time HH:MM, heure locale ;
-- schedule_days : bits 1 (lundi) à 7 (dimanche), 0 : une seule fois ; NULL : à la main),
-- prochain lancement prévu, tâche (une par projet, reprise à chaque lancement), dernier
-- lancement et dernière occurrence sautée (tâche encore en cours). public_only : créé par
-- une connexion à jeton.
CREATE TABLE projects (
  id              TEXT PRIMARY KEY,
  name            TEXT NOT NULL UNIQUE,
  page_url        TEXT NOT NULL,
  depth           INTEGER NOT NULL CHECK (depth BETWEEN 1 AND 5),
  max_pages       INTEGER NOT NULL CHECK (max_pages BETWEEN 1 AND 2000),
  exclude         TEXT,
  same_folder     INTEGER NOT NULL DEFAULT 0 CHECK (same_folder IN (0, 1)),
  ignore_robots   INTEGER NOT NULL DEFAULT 0 CHECK (ignore_robots IN (0, 1)),
  filter_ext      TEXT NOT NULL DEFAULT '',
  filter_regex    TEXT,
  filter_min      INTEGER CHECK (filter_min >= 0),
  filter_max      INTEGER CHECK (filter_max >= 0),
  copy_site       INTEGER NOT NULL DEFAULT 1 CHECK (copy_site IN (0, 1)),
  max_bytes       INTEGER NOT NULL CHECK (max_bytes > 0),
  keep_tree       INTEGER NOT NULL DEFAULT 1 CHECK (keep_tree IN (0, 1)),
  dest_dir        TEXT,
  queue_id        TEXT REFERENCES queues(id) ON DELETE SET NULL,
  schedule_time   TEXT,
  schedule_days   INTEGER NOT NULL DEFAULT 0 CHECK (schedule_days BETWEEN 0 AND 127),
  next_run_at     INTEGER,
  task_id         TEXT REFERENCES tasks(id) ON DELETE SET NULL,
  last_run_at     INTEGER,
  last_skipped_at INTEGER,
  public_only     INTEGER NOT NULL DEFAULT 0 CHECK (public_only IN (0, 1)),
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL
);
CREATE INDEX projects_next_run ON projects (next_run_at) WHERE next_run_at IS NOT NULL;
