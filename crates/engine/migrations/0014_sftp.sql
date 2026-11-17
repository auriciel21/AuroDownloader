-- SFTP (F-08, SPEC 7.22, phase 7, étape 6c, depuis le 2026-11-17).
--
-- Clés des serveurs SSH acceptées par l'utilisateur, une par `hôte:port` : l'empreinte
-- SHA-256 telle que l'affiche OpenSSH (`SHA256:…`). Une clé inconnue ou différente arrête
-- la tâche jusqu'à la décision de l'utilisateur ; le fichier `known_hosts` d'OpenSSH n'est
-- jamais lu.
CREATE TABLE ssh_host_keys (
  host        TEXT NOT NULL,
  port        INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
  algorithm   TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  accepted_at INTEGER NOT NULL,
  PRIMARY KEY (host, port)
);

-- Identifiants de site par clé privée (SFTP) : chemin du fichier choisi par l'utilisateur ;
-- `secret_ref` est alors sa phrase de passe (vide si la clé n'en a pas).
ALTER TABLE site_logins ADD COLUMN key_path TEXT;
