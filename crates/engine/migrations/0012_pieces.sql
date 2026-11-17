-- Empreintes de morceaux d'un Metalink (F-12, SPEC 7.9, phase 7, étape 5, depuis le
-- 2026-11-17) : seuls les morceaux faux se téléchargent de nouveau.
--
-- `hashes` : les empreintes de chaque morceau, dans l'ordre, en hexadécimal minuscule mises
-- bout à bout (chacune a la longueur de son algorithme). `rounds` : tours « morceaux faux
-- retéléchargés, puis vérification » faits depuis la dernière relance (3 au plus) ;
-- `redone` : morceaux retéléchargés en tout, pour l'affichage.
CREATE TABLE task_pieces (
  task_id TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
  algo    TEXT NOT NULL CHECK (algo IN ('sha256', 'sha1', 'md5')),
  length  INTEGER NOT NULL CHECK (length > 0),
  hashes  TEXT NOT NULL,
  rounds  INTEGER NOT NULL DEFAULT 0,
  redone  INTEGER NOT NULL DEFAULT 0
);

-- Morceaux faux envoyés par une source : à 2, elle est écartée pour sa tâche.
ALTER TABLE sources ADD COLUMN bad_pieces INTEGER NOT NULL DEFAULT 0;
