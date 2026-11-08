-- Origine de l'empreinte attendue (F-11, SPEC §7.9, depuis le 2026-11-08) : nom du
-- fichier voisin où le moteur l'a lue (« SHA256SUMS », « image.iso.sha256 »). NULL :
-- donnée par l'utilisateur, ou aucune empreinte. Un retéléchargement oublie une empreinte
-- lue ainsi : le fichier a pu changer depuis.
ALTER TABLE tasks ADD COLUMN checksum_source TEXT;
