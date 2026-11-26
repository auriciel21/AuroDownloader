-- Copie d'un site pour la navigation hors ligne (SPEC §7.18 ter, F-43, étape B).
--
-- La contrainte de tasks.kind reçoit 'site' avant ce script : la table tasks est refaite
-- par `store::rebuild_tasks_for_sites` (procédure en 12 étapes de SQLite, clés étrangères
-- coupées hors de la transaction), qui ne change que cette contrainte.

-- Réglages et avancement d'une copie. phase : copy (exploration, pages et ressources),
-- files (fichiers cochés), links (réécriture, en post_processing), done. stop : raison de
-- la fin de l'exploration (finished, maxPages, maxBytes, maxItems). start_url : page de
-- départ après redirections, start_path son chemin copié (relatif au dossier de la tâche).
CREATE TABLE site_jobs (
  task_id       TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
  page_url      TEXT NOT NULL,
  start_url     TEXT,
  start_path    TEXT,
  depth         INTEGER NOT NULL CHECK (depth BETWEEN 1 AND 5),
  max_pages     INTEGER NOT NULL CHECK (max_pages BETWEEN 1 AND 2000),
  max_bytes     INTEGER NOT NULL CHECK (max_bytes > 0),
  exclude       TEXT,
  same_folder   INTEGER NOT NULL DEFAULT 0 CHECK (same_folder IN (0, 1)),
  ignore_robots INTEGER NOT NULL DEFAULT 0 CHECK (ignore_robots IN (0, 1)),
  phase         TEXT NOT NULL DEFAULT 'copy' CHECK (phase IN ('copy','files','links','done')),
  stop          TEXT
);

-- Éléments d'une copie, dans l'ordre de leur découverte. state : queued (à lire), fetched
-- (page ou style lu, brut dans son .part vidé sur disque, ses liens relevés : attend la
-- réécriture), done (nom final pris et vidé sur disque), failed (lien laissé en ligne ;
-- error : code de la SPEC §6.8, ou TOO_BIG), skipped (limite atteinte avant sa lecture).
-- path : chemin pris, relatif au dossier de la tâche, parties séparées par '/'.
-- final_url : adresse après redirections. charset : encodage annoncé par l'en-tête
-- Content-Type d'une page, écrit en <meta charset> à la réécriture si la page ne dit pas
-- le sien. child_id : tâche HTTP interne d'un fichier coché.
CREATE TABLE site_items (
  task_id   TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  seq       INTEGER NOT NULL,
  url       TEXT NOT NULL,
  final_url TEXT,
  kind      TEXT NOT NULL CHECK (kind IN ('page','style','resource','file')),
  depth     INTEGER,
  state     TEXT NOT NULL DEFAULT 'queued'
              CHECK (state IN ('queued','fetched','done','failed','skipped')),
  path      TEXT,
  size      INTEGER,
  charset   TEXT,
  child_id  TEXT REFERENCES tasks(id) ON DELETE SET NULL,
  error     TEXT,
  PRIMARY KEY (task_id, seq),
  UNIQUE (task_id, url)
);
CREATE INDEX site_items_state ON site_items (task_id, state, seq);
