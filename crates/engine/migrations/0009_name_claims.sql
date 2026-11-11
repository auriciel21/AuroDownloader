-- Noms réservés sur le disque mais pas encore enregistrés (SPEC 5.6, règle 7, depuis le
-- 2026-11-11).
--
-- Un nom de fichier se réserve en créant son `.part` (ou le dossier d'un torrent à
-- plusieurs fichiers, ou le fichier d'un sous-titre) avant de l'enregistrer dans la tâche.
-- Un arrêt brutal entre les deux laissait un `.part` orphelin, et la tâche prenait le nom
-- suivant. Désormais la réservation est d'abord notée ici, puis effacée dans la
-- transaction qui enregistre le nom ; au démarrage, une réservation qui reste désigne ce
-- que le moteur a créé sans l'enregistrer : il est effacé.
CREATE TABLE name_claims (
  dir        TEXT NOT NULL,
  name       TEXT NOT NULL,          -- nom final ; le `.part` est `<name>.part`
  kind       TEXT NOT NULL CHECK (kind IN ('part', 'dir', 'file')),
  task_id    TEXT NOT NULL,          -- sans clé étrangère : survit au retrait de la tâche
  created_at INTEGER NOT NULL,
  PRIMARY KEY (dir, name)
);
CREATE INDEX name_claims_by_task ON name_claims (task_id);
