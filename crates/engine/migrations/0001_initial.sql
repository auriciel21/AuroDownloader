-- Migration 0001 : schéma initial (SPEC §5.2 à §5.4).
-- Générée à partir des blocs SQL de docs/SPEC.md ; les PRAGMA sont appliqués
-- à l'ouverture de la base (store.rs) et schema_migrations est créée par le lanceur.

CREATE TABLE settings (
  key        TEXT PRIMARY KEY,
  value_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE schedules (
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  rules_json TEXT NOT NULL,   -- plages horaires, jours, action de fin
  enabled    INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE queues (
  id              TEXT PRIMARY KEY,
  name            TEXT NOT NULL UNIQUE,
  position        INTEGER NOT NULL,
  max_active      INTEGER NOT NULL DEFAULT 3 CHECK (max_active >= 1),
  speed_limit_bps INTEGER CHECK (speed_limit_bps IS NULL OR speed_limit_bps > 0),
  schedule_id     TEXT REFERENCES schedules(id) ON DELETE SET NULL,
  created_at      INTEGER NOT NULL
);

CREATE TABLE rules (            -- rangement et renommage
  id           TEXT PRIMARY KEY,
  position     INTEGER NOT NULL,
  enabled      INTEGER NOT NULL DEFAULT 1,
  match_json   TEXT NOT NULL,   -- type, domaine, expression régulière
  actions_json TEXT NOT NULL    -- dossier cible, modèle de nom, commande
);

CREATE TABLE tasks (
  id              TEXT PRIMARY KEY,
  queue_id        TEXT NOT NULL REFERENCES queues(id),
  kind            TEXT NOT NULL CHECK (kind IN ('http','ftp','torrent','media')),
  state           TEXT NOT NULL CHECK (state IN ('queued','active','paused','retry_wait',
                    'verifying','post_processing','seeding','completed','failed','cancelled')),
  priority        INTEGER NOT NULL DEFAULT 0,
  position        INTEGER NOT NULL,
  display_name    TEXT NOT NULL,
  dest_dir        TEXT NOT NULL,
  file_name       TEXT,
  total_bytes     INTEGER,            -- NULL tant que la taille est inconnue
  done_bytes      INTEGER NOT NULL DEFAULT 0,
  max_connections INTEGER CHECK (max_connections BETWEEN 1 AND 32),  -- NULL = automatique
  speed_limit_bps INTEGER,
  checksum_algo   TEXT CHECK (checksum_algo IN ('sha256','sha1','md5')),
  checksum_value  TEXT,
  pause_reason    TEXT CHECK (pause_reason IN ('user','schedule','disk_full','network','file_changed')),
  error_code      TEXT,
  error_detail    TEXT,
  retry_count     INTEGER NOT NULL DEFAULT 0,
  next_retry_at   INTEGER,
  options_json    TEXT NOT NULL DEFAULT '{}',
  created_at      INTEGER NOT NULL,
  started_at      INTEGER,
  finished_at     INTEGER,
  updated_at      INTEGER NOT NULL
);

CREATE TABLE sources (
  id             TEXT PRIMARY KEY,
  task_id        TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  url            TEXT NOT NULL,
  rank           INTEGER NOT NULL DEFAULT 0,   -- ordre de préférence
  state          TEXT NOT NULL DEFAULT 'unknown' CHECK (state IN ('unknown','ok','slow','failed')),
  accepts_ranges INTEGER CHECK (accepts_ranges IN (0,1)),
  etag           TEXT,
  last_modified  TEXT,
  content_length INTEGER,
  avg_bps        INTEGER,
  fail_count     INTEGER NOT NULL DEFAULT 0,
  last_error     TEXT,
  UNIQUE (task_id, url)
);

CREATE TABLE segments (
  task_id       TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  idx           INTEGER NOT NULL,             -- croît à chaque redécoupage, jamais réutilisé
  start_offset  INTEGER NOT NULL,
  end_offset    INTEGER NOT NULL,             -- inclus
  written_bytes INTEGER NOT NULL DEFAULT 0,
  state         TEXT NOT NULL CHECK (state IN ('pending','active','done','failed')),
  source_id     TEXT REFERENCES sources(id) ON DELETE SET NULL,
  attempts      INTEGER NOT NULL DEFAULT 0,
  hash          BLOB,                         -- empreinte du segment si elle est connue
  PRIMARY KEY (task_id, idx),
  CHECK (end_offset >= start_offset),
  CHECK (written_bytes <= end_offset - start_offset + 1)
);

CREATE TABLE task_headers (
  task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  name       TEXT NOT NULL,
  value      TEXT,
  secret_ref TEXT,                            -- référence au coffre du système
  PRIMARY KEY (task_id, name),
  CHECK ((value IS NULL) <> (secret_ref IS NULL))
);

CREATE TABLE task_files (          -- tâches à plusieurs fichiers (torrent, lot)
  task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  file_index INTEGER NOT NULL,
  path       TEXT NOT NULL,
  length     INTEGER NOT NULL,
  selected   INTEGER NOT NULL DEFAULT 1,
  done_bytes INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (task_id, file_index)
);

CREATE TABLE torrents (
  task_id           TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
  info_hash         TEXT NOT NULL,
  magnet            TEXT,
  metainfo          BLOB,
  uploaded_bytes    INTEGER NOT NULL DEFAULT 0,
  ratio_limit       REAL,
  seed_time_limit_s INTEGER
);

CREATE TABLE media_jobs (
  task_id         TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
  page_url        TEXT NOT NULL,
  extractor       TEXT,
  format_selector TEXT NOT NULL,
  merge_container TEXT,
  subtitles_json  TEXT
);

CREATE TABLE history (              -- survit à la suppression de la tâche
  id          TEXT PRIMARY KEY,
  task_id     TEXT,
  display_name TEXT NOT NULL,
  final_path  TEXT,
  total_bytes INTEGER,
  source_host TEXT,
  final_state TEXT NOT NULL CHECK (final_state IN ('completed','failed','cancelled')),
  sha256      TEXT,
  finished_at INTEGER NOT NULL
);

CREATE TABLE event_log (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id TEXT,
  ts      INTEGER NOT NULL,
  level   TEXT NOT NULL CHECK (level IN ('debug','info','warn','error')),
  code    TEXT NOT NULL,
  message TEXT NOT NULL
);

CREATE INDEX idx_tasks_queue_state ON tasks (queue_id, state, priority DESC, position);
CREATE INDEX idx_segments_state    ON segments (task_id, state);
CREATE INDEX idx_sources_task      ON sources (task_id, rank);
CREATE INDEX idx_history_finished  ON history (finished_at DESC);
CREATE INDEX idx_event_log_task    ON event_log (task_id, ts);
