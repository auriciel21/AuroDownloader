-- Copie d'un site (SPEC §7.18 ter, décision de l'utilisateur du 2026-11-27) : les liens
-- laissés en ligne volontairement sont enregistrés à part des échecs techniques.
-- state : ignored (lien non copié par choix ; error : sa raison, ROBOTS, EXCLUDED, OFFSITE,
-- DEPTH, FOLDER, NOT_TICKED, LIMIT, NOT_HTML, REDIRECTED ou TOO_BIG). Les éléments en
-- échec NOT_HTML, REDIRECTED et TOO_BIG d'avant y passent.
-- site_jobs.ignored_over : liens ignorés au-delà de ceux enregistrés (10 000 au plus par
-- copie), seulement comptés.
--
-- SQLite ne change pas une contrainte : site_items est refaite. Aucune table ne la
-- référence ; ses clés étrangères vers tasks restent vérifiées à la copie.
CREATE TABLE site_items_new (
  task_id   TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  seq       INTEGER NOT NULL,
  url       TEXT NOT NULL,
  final_url TEXT,
  kind      TEXT NOT NULL CHECK (kind IN ('page','style','resource','file')),
  depth     INTEGER,
  state     TEXT NOT NULL DEFAULT 'queued'
              CHECK (state IN ('queued','fetched','done','failed','skipped','ignored')),
  path      TEXT,
  size      INTEGER,
  charset   TEXT,
  child_id  TEXT REFERENCES tasks(id) ON DELETE SET NULL,
  error     TEXT,
  PRIMARY KEY (task_id, seq),
  UNIQUE (task_id, url)
);
INSERT INTO site_items_new
  (task_id, seq, url, final_url, kind, depth, state, path, size, charset, child_id, error)
SELECT task_id, seq, url, final_url, kind, depth,
       CASE WHEN state = 'failed' AND error IN ('NOT_HTML', 'REDIRECTED', 'TOO_BIG')
            THEN 'ignored' ELSE state END,
       path, size, charset, child_id, error
FROM site_items;
DROP TABLE site_items;
ALTER TABLE site_items_new RENAME TO site_items;
CREATE INDEX site_items_state ON site_items (task_id, state, seq);

ALTER TABLE site_jobs ADD COLUMN ignored_over INTEGER NOT NULL DEFAULT 0;
