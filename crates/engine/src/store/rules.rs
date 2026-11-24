//! Règles de rangement en base (F-32, F-34, SPEC §7.21) : table `rules` (critères et
//! actions en JSON, au format du protocole), et rangement retenu pour une tâche
//! (`tasks.rule_base`, `tasks.rule_id`, migration 11).

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::Value;

use super::tasks::TaskRow;
use super::{StoreError, new_id};
use crate::rules::{Actions, Criteria, Placement, Rule, Scope, Subject};

fn parse(text: &str, what: &str) -> Result<Value, StoreError> {
    serde_json::from_str(text).map_err(|e| StoreError::BadValue(format!("règle, {what} : {e}")))
}

/// Les règles, dans leur ordre.
pub fn list(conn: &Connection) -> Result<Vec<Rule>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, enabled, match_json, actions_json FROM rules ORDER BY position, id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(id, name, enabled, criteria, actions)| {
            let criteria = Criteria::from_json(&parse(&criteria, "match_json")?)
                .map_err(StoreError::BadValue)?;
            let actions = Actions::from_json(&parse(&actions, "actions_json")?)
                .map_err(StoreError::BadValue)?;
            Ok(Rule {
                id,
                name,
                enabled,
                criteria,
                actions,
            })
        })
        .collect()
}

/// Une règle par son identifiant.
pub fn get(conn: &Connection, id: &str) -> Result<Option<Rule>, StoreError> {
    Ok(list(conn)?.into_iter().find(|r| r.id == id))
}

/// Remplace toutes les règles, dans l'ordre donné ; une règle sans identifiant en
/// reçoit un. Rend la liste enregistrée.
pub fn replace_all(conn: &mut Connection, rules: &[Rule]) -> Result<Vec<Rule>, StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM rules", [])?;
    let mut saved = Vec::with_capacity(rules.len());
    for (position, rule) in rules.iter().enumerate() {
        let mut rule = rule.clone();
        if rule.id.is_empty() {
            rule.id = new_id();
        }
        tx.execute(
            "INSERT INTO rules (id, position, enabled, match_json, actions_json, name)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                rule.id,
                position as i64,
                rule.enabled,
                rule.criteria.to_json().to_string(),
                rule.actions.to_json(true).to_string(),
                rule.name,
            ],
        )?;
        saved.push(rule);
    }
    tx.commit()?;
    Ok(saved)
}

/// Ce que le rangement d'une tâche doit savoir d'elle : le dossier des téléchargements de
/// son ajout si elle est venue sans dossier (`None` sinon), et le jour de son ajout en
/// heure locale (`AAAA-MM-JJ`).
fn task_scope(
    conn: &Connection,
    task_id: &str,
) -> Result<(Option<String>, String, bool), StoreError> {
    conn.query_row(
        "SELECT rule_base, date(created_at / 1000, 'unixepoch', 'localtime'),
                parent_id IS NOT NULL
         FROM tasks WHERE id = ?1",
        [task_id],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        },
    )
    .optional()?
    .ok_or_else(|| StoreError::TaskNotFound {
        task_id: task_id.to_owned(),
    })
}

/// Marque une tâche ajoutée sans dossier : les règles peuvent la ranger, sous son dossier
/// d'alors (celui des téléchargements). Un retéléchargement la range de nouveau.
pub fn set_rule_base(conn: &Connection, task_id: &str) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE tasks SET rule_base = dest_dir WHERE id = ?1",
        [task_id],
    )?;
    Ok(())
}

/// Règle retenue pour une tâche.
pub fn task_rule(conn: &Connection, task_id: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT rule_id FROM tasks WHERE id = ?1",
            [task_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// Rangement d'une tâche à sa préparation (SPEC §7.21) : la première règle activée qui
/// prend `subject`. `rename` : le modèle de nom peut s'appliquer (pas un torrent) ; un nom
/// choisi par l'utilisateur l'emporte toujours.
pub fn place_task(
    conn: &Connection,
    task: &TaskRow,
    subject: &Subject<'_>,
    rename: bool,
) -> Result<Placement, StoreError> {
    let rules = list(conn)?;
    if rules.is_empty() {
        return Ok(Placement::default());
    }
    let (base, date, internal) = task_scope(conn, &task.id)?;
    // Un flux d'une vidéo n'a pas de règle : sa vidéo en a une.
    if internal {
        return Ok(Placement::default());
    }
    Ok(crate::rules::place(
        &rules,
        subject,
        &Scope {
            auto_dest: base.is_some(),
            base: Path::new(base.as_deref().unwrap_or(&task.dest_dir)),
            rename: rename && task.requested_name.is_none(),
            date: &date,
        },
    ))
}

/// Rangement à enregistrer avec la préparation d'une tâche.
#[derive(Debug, Clone, Copy, Default)]
pub struct Placed<'a> {
    /// Règle retenue.
    pub rule_id: Option<&'a str>,
    /// Nouveau dossier ; `None` : inchangé.
    pub dest_dir: Option<&'a str>,
}

impl<'a> Placed<'a> {
    pub fn of(placement: &'a Placement, dest_dir: &'a str) -> Self {
        Placed {
            rule_id: placement.rule_id.as_deref(),
            dest_dir: placement.dir.is_some().then_some(dest_dir),
        }
    }
}

/// Enregistre le rangement d'une tâche, dans la transaction de sa préparation : la règle
/// retenue, et le nouveau dossier s'il change.
pub fn save_placement(
    tx: &Transaction<'_>,
    task_id: &str,
    placed: &Placed<'_>,
) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE tasks SET rule_id = ?2, dest_dir = COALESCE(?3, dest_dir)
         WHERE id = ?1",
        params![task_id, placed.rule_id, placed.dest_dir],
    )?;
    Ok(())
}
