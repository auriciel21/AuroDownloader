-- Plugins d'hébergeurs (F-41, SPEC 7.20, phase 6, étape 4, depuis le 2026-11-12).
--
-- Un plugin installé : son manifeste, l'empreinte SHA-256 de son fichier (gardé sous
-- `plugins/<empreinte>.wasm` à côté de la base, revérifié à chaque chargement), et les
-- permissions acceptées par l'utilisateur à l'installation (`sites`, `fetch` : listes JSON
-- de noms de domaine).
CREATE TABLE plugins (
  id           TEXT PRIMARY KEY,
  name         TEXT NOT NULL,
  version      TEXT NOT NULL,
  author       TEXT,
  description  TEXT,
  digest       TEXT NOT NULL,
  sites        TEXT NOT NULL,
  fetch        TEXT NOT NULL,
  enabled      INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  installed_at INTEGER NOT NULL
);

-- Tâche dont le lien direct vient d'un plugin : la page d'origine et le plugin, pour
-- redemander un lien neuf quand celui-ci a expiré (SPEC 7.20).
ALTER TABLE tasks ADD COLUMN plugin_page TEXT;
ALTER TABLE tasks ADD COLUMN plugin_id TEXT;
