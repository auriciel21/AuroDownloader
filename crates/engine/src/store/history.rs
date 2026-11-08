//! Historique des téléchargements (F-35, SPEC §5.4) : une ligne par fin de tâche
//! (terminée, en échec ou annulée), qui survit au retrait de la tâche. Lecture des plus
//! récentes d'abord, avec recherche et pages ; effacement, tout ou avant une date.

use rusqlite::{Connection, params};

use super::StoreError;

/// Une entrée de l'historique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRow {
    pub id: String,
    /// Tâche d'origine ; elle peut avoir été retirée depuis.
    pub task_id: Option<String>,
    pub display_name: String,
    /// Chemin du fichier final (tâche terminée) ; il suit « Déplacer/Renommer ».
    pub final_path: Option<String>,
    pub total_bytes: Option<u64>,
    /// Nom d'hôte de l'adresse, jamais l'adresse entière (SPEC §8.2).
    pub source_host: Option<String>,
    /// `completed`, `failed` ou `cancelled`.
    pub final_state: String,
    /// Empreinte SHA-256 vérifiée à la fin (F-11), s'il y en avait une.
    pub sha256: Option<String>,
    /// Millisecondes Unix.
    pub finished_at: i64,
}

/// Position dans la liste, après laquelle reprend la page suivante : la date de fin et
/// l'identifiant de la dernière entrée lue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub finished_at: i64,
    pub id: String,
}

impl Cursor {
    /// Forme échangée par le protocole : « date:identifiant ».
    pub fn text(&self) -> String {
        format!("{}:{}", self.finished_at, self.id)
    }

    pub fn parse(text: &str) -> Option<Self> {
        let (at, id) = text.split_once(':')?;
        Some(Cursor {
            finished_at: at.parse().ok()?,
            id: id.to_owned(),
        })
    }
}

/// Filtre de lecture de l'historique.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryFilter {
    /// Texte cherché, sans égard à la casse (lettres ASCII), dans le nom, le chemin et
    /// l'hôte.
    pub query: Option<String>,
    /// Seulement les entrées finies strictement avant (millisecondes Unix).
    pub before: Option<i64>,
    /// Suite d'une page précédente.
    pub cursor: Option<Cursor>,
    pub limit: Option<u32>,
}

/// Entrées de l'historique, les plus récentes d'abord.
pub fn list(conn: &Connection, filter: &HistoryFilter) -> Result<Vec<HistoryRow>, StoreError> {
    // `instr` plutôt que `LIKE` : ni `%` ni `_` à échapper dans le texte cherché.
    let mut stmt = conn.prepare(
        "SELECT id, task_id, display_name, final_path, total_bytes, source_host, final_state,
                sha256, finished_at
         FROM history
         WHERE (?1 IS NULL
                OR instr(lower(display_name), lower(?1)) > 0
                OR instr(lower(COALESCE(final_path, '')), lower(?1)) > 0
                OR instr(lower(COALESCE(source_host, '')), lower(?1)) > 0)
           AND (?2 IS NULL OR finished_at < ?2)
           AND (?3 IS NULL OR finished_at < ?3 OR (finished_at = ?3 AND id < ?4))
         ORDER BY finished_at DESC, id DESC
         LIMIT ?5",
    )?;
    let query = filter.query.as_deref().filter(|q| !q.trim().is_empty());
    let limit = filter.limit.map_or(-1, i64::from);
    let rows = stmt.query_map(
        params![
            query,
            filter.before,
            filter.cursor.as_ref().map(|c| c.finished_at),
            filter.cursor.as_ref().map(|c| c.id.as_str()),
            limit
        ],
        |r| {
            Ok((
                HistoryRow {
                    id: r.get(0)?,
                    task_id: r.get(1)?,
                    display_name: r.get(2)?,
                    final_path: r.get(3)?,
                    total_bytes: None,
                    source_host: r.get(5)?,
                    final_state: r.get(6)?,
                    sha256: r.get(7)?,
                    finished_at: r.get(8)?,
                },
                r.get::<_, Option<i64>>(4)?,
            ))
        },
    )?;
    rows.map(|row| {
        let (mut entry, total) = row?;
        entry.total_bytes = total
            .map(|t| {
                u64::try_from(t)
                    .map_err(|_| StoreError::BadValue(format!("historique {} : taille", entry.id)))
            })
            .transpose()?;
        Ok(entry)
    })
    .collect()
}

/// Efface l'historique, entièrement, ou seulement les entrées finies avant `before`
/// (millisecondes Unix) ; renvoie le nombre d'entrées effacées. Les tâches ne sont pas
/// touchées.
pub fn clear(conn: &Connection, before: Option<i64>) -> Result<u64, StoreError> {
    let removed = conn.execute(
        "DELETE FROM history WHERE ?1 IS NULL OR finished_at < ?1",
        [before],
    )?;
    Ok(removed as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_in_memory;

    fn insert(conn: &Connection, id: &str, name: &str, host: &str, state: &str, at: i64) {
        conn.execute(
            "INSERT INTO history (id, task_id, display_name, final_path, total_bytes,
                                  source_host, final_state, finished_at)
             VALUES (?1, NULL, ?2, ?3, 42, ?4, ?5, ?6)",
            params![id, name, format!("/dl/{name}"), host, state, at],
        )
        .unwrap();
    }

    fn ids(rows: &[HistoryRow]) -> Vec<&str> {
        rows.iter().map(|r| r.id.as_str()).collect()
    }

    #[test]
    fn newest_first_searched_and_paged() {
        let conn = open_in_memory().unwrap();
        insert(
            &conn,
            "a",
            "Debian.iso",
            "cdimage.debian.org",
            "completed",
            100,
        );
        insert(&conn, "b", "notes.pdf", "exemple.org", "failed", 200);
        // Même date : l'identifiant départage, sans rien perdre d'une page à l'autre.
        insert(&conn, "c", "photo.jpg", "exemple.org", "cancelled", 200);
        insert(&conn, "d", "musique.flac", "son.example", "completed", 300);

        let all = list(&conn, &HistoryFilter::default()).unwrap();
        assert_eq!(ids(&all), ["d", "c", "b", "a"]);
        assert_eq!(all[0].total_bytes, Some(42));
        assert_eq!(all[0].final_path.as_deref(), Some("/dl/musique.flac"));

        // Nom, chemin ou hôte, sans égard à la casse.
        let search = |q: &str| {
            let filter = HistoryFilter {
                query: Some(q.into()),
                ..HistoryFilter::default()
            };
            ids(&list(&conn, &filter).unwrap())
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(search("DEBIAN"), ["a"]);
        assert_eq!(search("exemple.org"), ["c", "b"]);
        assert_eq!(search("/dl/n"), ["b"]);
        assert_eq!(search("100%"), Vec::<String>::new(), "% n'est pas un joker");
        assert_eq!(search("  "), ["d", "c", "b", "a"], "recherche vide : tout");

        let before = HistoryFilter {
            before: Some(200),
            ..HistoryFilter::default()
        };
        assert_eq!(ids(&list(&conn, &before).unwrap()), ["a"]);

        // Pages de deux, chacune reprenant après la dernière entrée de la précédente.
        let mut pages = Vec::new();
        let mut cursor = None;
        loop {
            let filter = HistoryFilter {
                cursor: cursor.clone(),
                limit: Some(2),
                ..HistoryFilter::default()
            };
            let page = list(&conn, &filter).unwrap();
            if page.is_empty() {
                break;
            }
            let last = page.last().unwrap();
            cursor = Cursor::parse(
                &Cursor {
                    finished_at: last.finished_at,
                    id: last.id.clone(),
                }
                .text(),
            );
            pages.push(ids(&page).join(","));
        }
        assert_eq!(pages, ["d,c", "b,a"]);
        assert_eq!(Cursor::parse("abc"), None);
        assert_eq!(Cursor::parse("x:y"), None);
    }

    #[test]
    fn clearing_everything_or_what_ended_before_a_date() {
        let conn = open_in_memory().unwrap();
        for (id, at) in [("a", 100), ("b", 200), ("c", 300)] {
            insert(&conn, id, id, "h", "completed", at);
        }
        assert_eq!(clear(&conn, Some(250)).unwrap(), 2);
        assert_eq!(ids(&list(&conn, &HistoryFilter::default()).unwrap()), ["c"]);
        assert_eq!(clear(&conn, None).unwrap(), 1);
        assert!(list(&conn, &HistoryFilter::default()).unwrap().is_empty());
    }
}
