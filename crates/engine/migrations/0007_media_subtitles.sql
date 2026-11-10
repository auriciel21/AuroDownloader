-- Sous-titres des vidéos (SPEC 7.17, F-24, étape 4e, depuis le 2026-11-09).
--
-- Un sous-titre est une ligne de `media_streams` après les flux de la vidéo, avec sa
-- langue : téléchargé par une tâche HTTP interne comme un flux direct, mais gardé à côté
-- de la vidéo (`<nom>.<langue>.<ext>`), jamais fusionné. NULL : flux vidéo ou audio.
ALTER TABLE media_streams ADD COLUMN language TEXT;
