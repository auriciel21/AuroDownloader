-- Interface web distante (F-39, SPEC 7.19, phase 6, étape 3, depuis le 2026-11-11).
--
-- Jetons des connexions réseau : la base n'en garde que l'empreinte SHA-256, jamais le
-- jeton, rendu une seule fois à sa création. Portée `read`, `add` ou `control`, jamais
-- `admin` (décision de l'utilisateur : `admin` reste au canal local).
CREATE TABLE remote_tokens (
  id           TEXT PRIMARY KEY,
  name         TEXT NOT NULL,
  scope        TEXT NOT NULL CHECK (scope IN ('read', 'add', 'control')),
  digest       BLOB NOT NULL UNIQUE,
  created_at   INTEGER NOT NULL,
  last_used_at INTEGER
);

-- Sessions des navigateurs, ouvertes par un jeton (`POST /login`) : empreinte de la
-- valeur du cookie seulement ; effacées avec leur jeton.
CREATE TABLE remote_sessions (
  digest     BLOB PRIMARY KEY,
  token_id   TEXT NOT NULL REFERENCES remote_tokens (id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE INDEX remote_sessions_by_token ON remote_sessions (token_id);

-- Tâche ajoutée par une connexion à jeton : elle ne joint que le réseau public, pour
-- toute sa vie, reprises comprises (protection SSRF).
ALTER TABLE tasks ADD COLUMN public_only INTEGER NOT NULL DEFAULT 0
  CHECK (public_only IN (0, 1));
