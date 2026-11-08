-- Identifiants de sites (F-06, SPEC §7.14, depuis le 2026-11-08) : nom d'utilisateur et
-- mot de passe d'un site, envoyés quand un serveur de ce site les demande (Basic, Digest).
-- Le mot de passe est au coffre du système ; la base n'en garde que la référence
-- (SPEC §5.1).
CREATE TABLE site_logins (
  id         TEXT PRIMARY KEY,
  site       TEXT NOT NULL UNIQUE,  -- forme normalisée : [protocole://]hôte[:port][chemin]
  username   TEXT NOT NULL,
  secret_ref TEXT NOT NULL,         -- mot de passe, au coffre
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
