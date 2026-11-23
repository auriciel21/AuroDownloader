-- Passage de HTTPS à HTTP (SPEC 8.1, audit T11, depuis le 2026-11-23).
--
-- 1 : la préparation de la tâche a suivi une redirection d'une adresse `https://` vers une
-- adresse `http://` ; le fichier arrive sans chiffrement. Noté à la préparation, annoncé par
-- `engine.warning` (`INSECURE_REDIRECT`) et rendu par `task.get` ; remis à 0 par un
-- retéléchargement, dont la préparation le note de nouveau.
ALTER TABLE tasks ADD COLUMN insecure_redirect INTEGER NOT NULL DEFAULT 0;
