//! Copies de sites pour la navigation hors ligne (SPEC §7.18 ter, F-43, migration 18) : la
//! ligne `site_jobs` (réglages, phase, raison de la fin de l'exploration) et les éléments
//! de la copie (`site_items` : pages, styles, ressources d'affichage, fichiers cochés).
//!
//! Un élément prend son chemin (`path`) dès sa réservation : [`set_path`] l'enregistre
//! dans la transaction qui efface la réservation (SPEC §5.6, règle 7). Il ne passe à
//! `fetched` ou `done` qu'une fois ses octets vidés sur disque (règle 1).

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::{StoreError, new_id, now_ms, to_i64};

/// Éléments d'une copie au plus (pages, ressources, fichiers ; SPEC §7.18 ter).
pub const MAX_ITEMS: usize = 20_000;

/// Liens ignorés enregistrés au plus par copie ; au-delà, seulement comptés
/// (`site_jobs.ignored_over`). Hors de [`MAX_ITEMS`].
pub const MAX_IGNORED: usize = 10_000;

/// Raisons d'un lien laissé en ligne par choix (`error` d'un élément `ignored`), à part
/// des échecs techniques (décision de l'utilisateur du 2026-11-27).
pub mod reason {
    /// Refusé par robots.txt.
    pub const ROBOTS: &str = "ROBOTS";
    /// Répond à l'expression des pages à ne pas ouvrir.
    pub const EXCLUDED: &str = "EXCLUDED";
    /// Page d'un autre site (ou de `https` vers `http`).
    pub const OFFSITE: &str = "OFFSITE";
    /// Au-delà de la profondeur choisie.
    pub const DEPTH: &str = "DEPTH";
    /// Hors du dossier de départ.
    pub const FOLDER: &str = "FOLDER";
    /// Lien vers un fichier que l'utilisateur n'a pas coché.
    pub const NOT_TICKED: &str = "NOT_TICKED";
    /// Trouvé après l'arrêt de l'exploration par une limite.
    pub const LIMIT: &str = "LIMIT";
    /// Lien de page qui mène à autre chose qu'une page web.
    pub const NOT_HTML: &str = "NOT_HTML";
    /// Page redirigée vers une page refusée.
    pub const REDIRECTED: &str = "REDIRECTED";
    /// Au-delà de la taille permise d'une page ou d'une ressource.
    pub const TOO_BIG: &str = "TOO_BIG";
}

/// Sorte d'un élément.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    /// Page du site, lue et réécrite.
    Page,
    /// Feuille de style, lue et réécrite (`url(…)`, `@import`).
    Style,
    /// Ressource d'affichage copiée telle quelle (image, script, police).
    Resource,
    /// Fichier coché par l'utilisateur, téléchargé par une tâche HTTP interne.
    File,
}

impl ItemKind {
    pub fn as_sql(self) -> &'static str {
        match self {
            ItemKind::Page => "page",
            ItemKind::Style => "style",
            ItemKind::Resource => "resource",
            ItemKind::File => "file",
        }
    }

    pub fn from_sql(s: &str) -> Option<Self> {
        match s {
            "page" => Some(ItemKind::Page),
            "style" => Some(ItemKind::Style),
            "resource" => Some(ItemKind::Resource),
            "file" => Some(ItemKind::File),
            _ => None,
        }
    }

    /// Lu puis réécrit à la fin (son brut reste dans son `.part` jusque-là).
    pub fn rewritten(self) -> bool {
        matches!(self, ItemKind::Page | ItemKind::Style)
    }
}

/// État d'un élément.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemState {
    Queued,
    /// Page ou style lu, brut vidé sur disque, liens relevés : attend la réécriture.
    Fetched,
    Done,
    /// Échec technique : lien laissé en ligne.
    Failed,
    /// Limite atteinte avant sa lecture.
    Skipped,
    /// Lien laissé en ligne par choix (robots.txt, exclusion, autre site…) : sa raison
    /// dans `error` ([`reason`]).
    Ignored,
}

impl ItemState {
    pub fn as_sql(self) -> &'static str {
        match self {
            ItemState::Queued => "queued",
            ItemState::Fetched => "fetched",
            ItemState::Done => "done",
            ItemState::Failed => "failed",
            ItemState::Skipped => "skipped",
            ItemState::Ignored => "ignored",
        }
    }

    pub fn from_sql(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(ItemState::Queued),
            "fetched" => Some(ItemState::Fetched),
            "done" => Some(ItemState::Done),
            "failed" => Some(ItemState::Failed),
            "skipped" => Some(ItemState::Skipped),
            "ignored" => Some(ItemState::Ignored),
            _ => None,
        }
    }
}

/// Phase d'une copie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Exploration, pages et ressources.
    Copy,
    /// Fichiers cochés.
    Files,
    /// Réécriture des liens (tâche en `post_processing`).
    Links,
    Done,
}

impl Phase {
    pub fn as_sql(self) -> &'static str {
        match self {
            Phase::Copy => "copy",
            Phase::Files => "files",
            Phase::Links => "links",
            Phase::Done => "done",
        }
    }

    pub fn from_sql(s: &str) -> Option<Self> {
        match s {
            "copy" => Some(Phase::Copy),
            "files" => Some(Phase::Files),
            "links" => Some(Phase::Links),
            "done" => Some(Phase::Done),
            _ => None,
        }
    }
}

/// Réglages d'une copie, ceux de l'exploration (SPEC §7.18 bis) et le volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub depth: u8,
    pub max_pages: u32,
    pub max_bytes: u64,
    pub exclude: Option<String>,
    pub same_folder: bool,
    pub ignore_robots: bool,
}

/// Une copie à créer.
#[derive(Debug, Clone)]
pub struct NewSite<'a> {
    pub queue_id: &'a str,
    pub page_url: &'a str,
    pub display_name: &'a str,
    pub dest_dir: &'a str,
    pub speed_limit_bps: Option<u64>,
    pub max_connections: Option<u8>,
    pub idempotency_key: Option<&'a str>,
    pub settings: &'a Settings,
    /// Fichiers cochés, adresses absolues sans doublon.
    pub files: &'a [String],
    /// Ajoutée par une connexion à jeton : réseau public seulement (SPEC §7.19).
    pub public_only: bool,
}

/// Crée une copie `queued`, sa source (la page de départ), sa ligne `site_jobs` et ses
/// premiers éléments : la page de départ (niveau 0), puis les fichiers cochés.
pub fn create_site_task(
    conn: &mut Connection,
    new: &NewSite<'_>,
    start: super::tasks::Start,
) -> Result<String, StoreError> {
    if new.files.len() + 1 > MAX_ITEMS {
        return Err(StoreError::BadValue("trop de fichiers cochés".into()));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (task_id, now) = (new_id(), now_ms());
    let limit = new
        .speed_limit_bps
        .map(|l| to_i64(l, "limite de débit"))
        .transpose()?;
    tx.execute(
        "INSERT INTO tasks (id, queue_id, kind, state, position, display_name, dest_dir,
                            max_connections, speed_limit_bps, idempotency_key, created_at,
                            updated_at, public_only)
         VALUES (?1, ?2, 'site', 'queued',
                 (SELECT COALESCE(MAX(position), -1) + 1 FROM tasks WHERE queue_id = ?2),
                 ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9)",
        params![
            task_id,
            new.queue_id,
            new.display_name,
            new.dest_dir,
            new.max_connections,
            limit,
            new.idempotency_key,
            now,
            new.public_only
        ],
    )?;
    tx.execute(
        "INSERT INTO sources (id, task_id, url) VALUES (?1, ?2, ?3)",
        params![new_id(), task_id, new.page_url],
    )?;
    let s = new.settings;
    tx.execute(
        "INSERT INTO site_jobs (task_id, page_url, depth, max_pages, max_bytes, exclude,
                                same_folder, ignore_robots)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            task_id,
            new.page_url,
            s.depth,
            s.max_pages,
            to_i64(s.max_bytes, "volume")?,
            s.exclude,
            s.same_folder,
            s.ignore_robots
        ],
    )?;
    insert_first_items(&tx, &task_id, new.page_url, new.files)?;
    super::tasks::apply_start(&tx, &task_id, start)?;
    tx.commit()?;
    Ok(task_id)
}

/// La page de départ (élément 0, niveau 0), puis les fichiers cochés.
fn insert_first_items(
    conn: &Connection,
    task_id: &str,
    page_url: &str,
    files: &[String],
) -> Result<(), StoreError> {
    let mut items = vec![NewItem {
        url: page_url,
        kind: ItemKind::Page,
        depth: Some(0),
        ignored: None,
    }];
    items.extend(files.iter().map(|url| NewItem {
        url,
        kind: ItemKind::File,
        depth: None,
        ignored: None,
    }));
    add_items(conn, task_id, &items)?;
    Ok(())
}

/// Ce que l'utilisateur a demandé, et où en est la copie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteJob {
    pub page_url: String,
    /// Page de départ après redirections, connue à sa première lecture.
    pub start_url: Option<String>,
    /// Chemin copié de la page de départ, relatif au dossier de la tâche.
    pub start_path: Option<String>,
    pub settings: Settings,
    pub phase: Phase,
    /// Raison de la fin de l'exploration (`finished`, `maxPages`, `maxBytes`, `maxItems`).
    pub stop: Option<String>,
}

fn bad(task_id: &str, what: &str) -> StoreError {
    StoreError::BadValue(format!("copie {task_id} : {what}"))
}

pub fn load_job(conn: &Connection, task_id: &str) -> Result<SiteJob, StoreError> {
    let row = conn
        .query_row(
            "SELECT page_url, start_url, start_path, depth, max_pages, max_bytes, exclude,
                    same_folder, ignore_robots, phase, stop
             FROM site_jobs WHERE task_id = ?1",
            [task_id],
            |r| {
                Ok((
                    (
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ),
                    (
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, bool>(7)?,
                        r.get::<_, bool>(8)?,
                    ),
                    r.get::<_, String>(9)?,
                    r.get::<_, Option<String>>(10)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| bad(task_id, "pas une copie de site"))?;
    let ((page_url, start_url, start_path), (depth, pages, bytes, exclude, folder, robots)) =
        (row.0, row.1);
    Ok(SiteJob {
        page_url,
        start_url,
        start_path,
        settings: Settings {
            depth: u8::try_from(depth).map_err(|_| bad(task_id, "profondeur"))?,
            max_pages: u32::try_from(pages).map_err(|_| bad(task_id, "pages"))?,
            max_bytes: u64::try_from(bytes).map_err(|_| bad(task_id, "volume"))?,
            exclude,
            same_folder: folder,
            ignore_robots: robots,
        },
        phase: Phase::from_sql(&row.2).ok_or_else(|| bad(task_id, "phase"))?,
        stop: row.3,
    })
}

/// Page de départ après redirections, et son chemin copié.
pub fn set_start(
    conn: &Connection,
    task_id: &str,
    start_url: &str,
    start_path: &str,
) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE site_jobs SET start_url = ?2, start_path = ?3 WHERE task_id = ?1",
        params![task_id, start_url, start_path],
    )?;
    Ok(())
}

pub fn set_phase(conn: &Connection, task_id: &str, phase: Phase) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE site_jobs SET phase = ?2 WHERE task_id = ?1",
        params![task_id, phase.as_sql()],
    )?;
    Ok(())
}

/// Raison de la fin de l'exploration ; la première notée reste.
pub fn set_stop(conn: &Connection, task_id: &str, stop: &str) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE site_jobs SET stop = COALESCE(stop, ?2) WHERE task_id = ?1",
        params![task_id, stop],
    )?;
    Ok(())
}

/// Un élément d'une copie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub seq: i64,
    pub url: String,
    pub final_url: Option<String>,
    pub kind: ItemKind,
    /// Niveau d'une page (0 pour la page de départ).
    pub depth: Option<u8>,
    pub state: ItemState,
    /// Chemin pris, relatif au dossier de la tâche, parties séparées par `/`.
    pub path: Option<String>,
    pub size: Option<u64>,
    pub child_id: Option<String>,
    pub error: Option<String>,
    /// Encodage annoncé par l'en-tête d'une page.
    pub charset: Option<String>,
}

const ITEM_COLUMNS: &str =
    "seq, url, final_url, kind, depth, state, path, size, child_id, error, charset";

fn read_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<Item> {
    let text = |i: usize, s: String| {
        rusqlite::Error::FromSqlConversionFailure(
            i,
            rusqlite::types::Type::Text,
            format!("valeur inconnue : {s}").into(),
        )
    };
    let kind: String = r.get(3)?;
    let state: String = r.get(5)?;
    Ok(Item {
        seq: r.get(0)?,
        url: r.get(1)?,
        final_url: r.get(2)?,
        kind: ItemKind::from_sql(&kind).ok_or_else(|| text(3, kind.clone()))?,
        depth: r
            .get::<_, Option<i64>>(4)?
            .map(|d| u8::try_from(d).unwrap_or(u8::MAX)),
        state: ItemState::from_sql(&state).ok_or_else(|| text(5, state.clone()))?,
        path: r.get(6)?,
        size: r
            .get::<_, Option<i64>>(7)?
            .map(|s| u64::try_from(s).unwrap_or(0)),
        child_id: r.get(8)?,
        error: r.get(9)?,
        charset: r.get(10)?,
    })
}

/// Tous les éléments d'une copie, dans l'ordre de leur découverte.
pub fn items(conn: &Connection, task_id: &str) -> Result<Vec<Item>, StoreError> {
    items_after(conn, task_id, -1)
}

/// Les éléments d'une copie venus après `seq`, dans l'ordre de leur découverte.
pub fn items_after(conn: &Connection, task_id: &str, seq: i64) -> Result<Vec<Item>, StoreError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {ITEM_COLUMNS} FROM site_items WHERE task_id = ?1 AND seq > ?2 ORDER BY seq"
    ))?;
    let rows = stmt
        .query_map(params![task_id, seq], read_item)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn item(conn: &Connection, task_id: &str, seq: i64) -> Result<Item, StoreError> {
    conn.query_row(
        &format!("SELECT {ITEM_COLUMNS} FROM site_items WHERE task_id = ?1 AND seq = ?2"),
        params![task_id, seq],
        read_item,
    )
    .optional()?
    .ok_or_else(|| bad(task_id, "élément inconnu"))
}

/// Un élément trouvé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewItem<'a> {
    pub url: &'a str,
    pub kind: ItemKind,
    pub depth: Option<u8>,
    /// Lien laissé en ligne par choix : sa raison ([`reason`]) ; `None` pour un élément à
    /// copier.
    pub ignored: Option<&'static str>,
}

/// Issue d'un ajout d'éléments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Added {
    /// Éléments nouveaux (une adresse déjà connue de la copie ne compte pas).
    pub added: usize,
    /// Des éléments n'ont pas été ajoutés : [`MAX_ITEMS`] atteint.
    pub full: bool,
    /// Liens ignorés nouveaux enregistrés.
    pub ignored: usize,
}

/// Ajoute des éléments à la suite, sans doublon d'adresse, jusqu'à [`MAX_ITEMS`]. Un lien
/// ignoré s'enregistre jusqu'à [`MAX_IGNORED`], puis se compte seulement ; un élément à
/// copier prend la place d'un lien ignoré de même adresse (une page trop profonde depuis
/// une page peut se suivre depuis une autre), avec un numéro neuf.
pub fn add_items(
    conn: &Connection,
    task_id: &str,
    items: &[NewItem<'_>],
) -> Result<Added, StoreError> {
    let (mut count, mut ignored, mut next): (i64, i64, i64) = conn.query_row(
        "SELECT COUNT(*) FILTER (WHERE state != 'ignored'),
                COUNT(*) FILTER (WHERE state = 'ignored'),
                COALESCE(MAX(seq), -1) + 1
         FROM site_items WHERE task_id = ?1",
        [task_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let mut added = Added::default();
    let mut stmt = conn.prepare_cached(
        "INSERT OR IGNORE INTO site_items (task_id, seq, url, kind, depth, state, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    let mut over: i64 = 0;
    for item in items {
        if let Some(why) = item.ignored {
            if usize::try_from(ignored).unwrap_or(usize::MAX) >= MAX_IGNORED {
                let known: bool = conn.query_row(
                    "SELECT EXISTS (SELECT 1 FROM site_items WHERE task_id = ?1 AND url = ?2)",
                    params![task_id, item.url],
                    |r| r.get(0),
                )?;
                over += i64::from(!known);
                continue;
            }
            if stmt.execute(params![
                task_id,
                next,
                item.url,
                item.kind.as_sql(),
                item.depth,
                ItemState::Ignored.as_sql(),
                why
            ])? == 1
            {
                next += 1;
                ignored += 1;
                added.ignored += 1;
            }
            continue;
        }
        let replaced = conn.execute(
            "DELETE FROM site_items WHERE task_id = ?1 AND url = ?2 AND state = 'ignored'",
            params![task_id, item.url],
        )?;
        if replaced > 0 {
            ignored -= 1;
        }
        if usize::try_from(count).unwrap_or(usize::MAX) >= MAX_ITEMS {
            // Une adresse déjà connue ne compte pas comme refusée.
            let known: bool = conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM site_items WHERE task_id = ?1 AND url = ?2)",
                params![task_id, item.url],
                |r| r.get(0),
            )?;
            added.full |= !known;
            continue;
        }
        if stmt.execute(params![
            task_id,
            next,
            item.url,
            item.kind.as_sql(),
            item.depth,
            ItemState::Queued.as_sql(),
            None::<&str>
        ])? == 1
        {
            next += 1;
            count += 1;
            added.added += 1;
        }
    }
    if over > 0 {
        conn.execute(
            "UPDATE site_jobs SET ignored_over = ignored_over + ?2 WHERE task_id = ?1",
            params![task_id, over],
        )?;
    }
    Ok(added)
}

/// Enregistre le chemin réservé d'un élément, et efface la réservation dans la même
/// transaction (SPEC §5.6, règle 7).
pub fn set_path(
    conn: &mut Connection,
    task_id: &str,
    seq: i64,
    path: &str,
) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "UPDATE site_items SET path = ?3 WHERE task_id = ?1 AND seq = ?2",
        params![task_id, seq, path],
    )?;
    super::claims::release(&tx, task_id)?;
    tx.commit()?;
    Ok(())
}

/// Une page ou un style lu, son brut vidé sur disque : ses liens trouvés sont mis en
/// attente dans la même transaction.
pub fn fetched(
    conn: &mut Connection,
    task_id: &str,
    seq: i64,
    final_url: Option<&str>,
    size: u64,
    charset: Option<&str>,
    found: &[NewItem<'_>],
) -> Result<Added, StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "UPDATE site_items SET state = 'fetched', final_url = ?3, size = ?4, charset = ?5,
                               error = NULL
         WHERE task_id = ?1 AND seq = ?2",
        params![task_id, seq, final_url, to_i64(size, "taille")?, charset],
    )?;
    let added = add_items(&tx, task_id, found)?;
    tx.commit()?;
    Ok(added)
}

/// Un élément à son nom final, vidé sur disque.
pub fn done(
    conn: &Connection,
    task_id: &str,
    seq: i64,
    final_url: Option<&str>,
    size: u64,
) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE site_items
         SET state = 'done', final_url = COALESCE(?3, final_url), size = ?4, error = NULL
         WHERE task_id = ?1 AND seq = ?2",
        params![task_id, seq, final_url, to_i64(size, "taille")?],
    )?;
    Ok(())
}

/// Un élément laissé en ligne : en échec (avec son code), écarté par une limite, ou
/// ignoré par choix (avec sa raison).
pub fn give_up(
    conn: &Connection,
    task_id: &str,
    seq: i64,
    state: ItemState,
    error: Option<&str>,
) -> Result<(), StoreError> {
    debug_assert!(matches!(
        state,
        ItemState::Failed | ItemState::Skipped | ItemState::Ignored
    ));
    conn.execute(
        "UPDATE site_items SET state = ?3, error = ?4 WHERE task_id = ?1 AND seq = ?2",
        params![task_id, seq, state.as_sql(), error],
    )?;
    Ok(())
}

/// Limite atteinte : les éléments encore en attente sont écartés, les fichiers cochés
/// gardés ; avec `pages_only` (limite de pages ou d'éléments), seulement les pages : les
/// ressources des pages déjà lues restent à copier. Rend leur nombre.
pub fn skip_queued(
    conn: &Connection,
    task_id: &str,
    pages_only: bool,
) -> Result<usize, StoreError> {
    let kinds = if pages_only {
        "('page')"
    } else {
        "('page', 'style', 'resource')"
    };
    Ok(conn.execute(
        &format!(
            "UPDATE site_items SET state = 'skipped'
             WHERE task_id = ?1 AND state = 'queued' AND kind IN {kinds}"
        ),
        [task_id],
    )?)
}

/// Le dossier de la copie (`tasks.file_name`), réservé dans le dossier choisi :
/// enregistré dans la transaction qui efface la réservation (SPEC §5.6, règle 7).
pub fn set_root(conn: &mut Connection, task_id: &str, name: &str) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "UPDATE tasks SET file_name = ?2, updated_at = ?3 WHERE id = ?1",
        params![task_id, name, now_ms()],
    )?;
    super::claims::release(&tx, task_id)?;
    tx.commit()?;
    Ok(())
}

/// Octets écrits par la copie (pages, ressources et fichiers cochés faits), pour
/// l'avancement.
pub fn save_progress(conn: &Connection, task_id: &str, done_bytes: u64) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE tasks SET done_bytes = ?2, updated_at = ?3 WHERE id = ?1",
        params![task_id, to_i64(done_bytes, "octets")?, now_ms()],
    )?;
    Ok(())
}

/// Tâche HTTP interne d'un fichier coché.
pub fn set_child(
    conn: &Connection,
    task_id: &str,
    seq: i64,
    child_id: &str,
) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE site_items SET child_id = ?3 WHERE task_id = ?1 AND seq = ?2",
        params![task_id, seq, child_id],
    )?;
    Ok(())
}

/// Tâches HTTP internes d'une copie.
pub fn children(conn: &Connection, task_id: &str) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare("SELECT id FROM tasks WHERE parent_id = ?1 ORDER BY id")?;
    let ids = stmt
        .query_map([task_id], |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(ids)
}

/// Retéléchargement (`task.restart`) : la copie repart de zéro, avec ses fichiers cochés ;
/// ses tâches internes sont retirées. Les fichiers sur le disque sont l'affaire de
/// l'appelant, avant.
pub fn reset(conn: &mut Connection, task_id: &str) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let job = load_job(&tx, task_id)?;
    let files: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT url FROM site_items WHERE task_id = ?1 AND kind = 'file' ORDER BY seq",
        )?;
        stmt.query_map([task_id], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    tx.execute("DELETE FROM site_items WHERE task_id = ?1", [task_id])?;
    tx.execute("DELETE FROM tasks WHERE parent_id = ?1", [task_id])?;
    tx.execute(
        "UPDATE site_jobs SET start_url = NULL, start_path = NULL, phase = 'copy', stop = NULL,
                             ignored_over = 0
         WHERE task_id = ?1",
        [task_id],
    )?;
    insert_first_items(&tx, task_id, &job.page_url, &files)?;
    tx.commit()?;
    Ok(())
}

/// Nombres d'une copie (SPEC §7.18 ter : `task.get`, `task.progress`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Pages lues (en attente de réécriture ou faites).
    pub pages: u64,
    /// Styles et ressources d'affichage copiés.
    pub resources: u64,
    /// Fichiers cochés copiés.
    pub files: u64,
    /// Échecs techniques (lien laissé en ligne).
    pub failed: u64,
    /// Ignorés, laissés en ligne par choix (décision de l'utilisateur du 2026-11-27) :
    /// éléments `ignored` et `skipped`, et liens comptés au-delà de [`MAX_IGNORED`].
    pub ignored: u64,
    /// Dont ceux qui attendaient quand une limite a arrêté l'exploration.
    pub skipped: u64,
    pub queued: u64,
    /// Octets des pages et ressources enregistrées (le volume limité, hors fichiers
    /// cochés).
    pub bytes: u64,
}

pub fn counts(conn: &Connection, task_id: &str) -> Result<Counts, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT kind, state, COUNT(*), COALESCE(SUM(size), 0)
         FROM site_items WHERE task_id = ?1 GROUP BY kind, state",
    )?;
    let rows = stmt.query_map([task_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    let over: i64 = conn
        .query_row(
            "SELECT ignored_over FROM site_jobs WHERE task_id = ?1",
            [task_id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let mut c = Counts {
        ignored: u64::try_from(over).unwrap_or(0),
        ..Counts::default()
    };
    for row in rows {
        let (kind, state, n, size) = row?;
        let n = u64::try_from(n).unwrap_or(0);
        let size = u64::try_from(size).unwrap_or(0);
        let kind = ItemKind::from_sql(&kind).ok_or_else(|| bad(task_id, "sorte"))?;
        let state = ItemState::from_sql(&state).ok_or_else(|| bad(task_id, "état"))?;
        match state {
            ItemState::Queued => c.queued += n,
            ItemState::Failed => c.failed += n,
            ItemState::Ignored => c.ignored += n,
            ItemState::Skipped => {
                c.skipped += n;
                c.ignored += n;
            }
            ItemState::Fetched | ItemState::Done => {
                match kind {
                    ItemKind::Page => c.pages += n,
                    ItemKind::Style | ItemKind::Resource => c.resources += n,
                    ItemKind::File => c.files += n,
                }
                if kind != ItemKind::File {
                    c.bytes += size;
                }
            }
        }
    }
    Ok(c)
}

#[cfg(test)]
mod tests;
