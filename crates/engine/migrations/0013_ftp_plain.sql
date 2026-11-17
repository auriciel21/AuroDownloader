-- FTP sans chiffrement permis pour un site (F-08, SPEC 7.22, phase 7, étape 6b, depuis le
-- 2026-11-17) : « autoriser sans chiffrement ». Faux par défaut : avec un mot de passe, le
-- moteur exige TLS (`AUTH TLS`) et refuse un serveur qui ne le propose pas.
ALTER TABLE site_logins ADD COLUMN allow_plain INTEGER NOT NULL DEFAULT 0
  CHECK (allow_plain IN (0, 1));
