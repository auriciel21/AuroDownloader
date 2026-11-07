-- Clé d'idempotence de task.add (SPEC §6.1) : rejouer un ajout avec la même clé rend la
-- tâche déjà créée au lieu d'en créer une seconde (l'extension navigateur réessaie).
-- Retirer la tâche libère la clé.
ALTER TABLE tasks ADD COLUMN idempotency_key TEXT;

CREATE UNIQUE INDEX tasks_idempotency_key ON tasks (idempotency_key)
    WHERE idempotency_key IS NOT NULL;
