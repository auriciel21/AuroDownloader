-- Durée d'une vidéo (SPEC 7.17, audit V14 et V15, depuis le 2026-11-24).
--
-- Durée annoncée par la page à la résolution, en secondes ; NULL si elle ne la donne pas.
-- Gardée pour contrôler une fusion relancée après un arrêt (ffprobe), et pour finir une
-- vidéo dont tous les flux sont faits quand la page ne se relit plus.
ALTER TABLE media_jobs ADD COLUMN duration_s REAL;
