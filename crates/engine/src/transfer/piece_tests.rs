//! Morceaux faux d'un Metalink (F-12, SPEC §7.9, phase 7, étape 5) : seuls les morceaux
//! faux se téléchargent de nouveau, de préférence d'une autre source ; trois tours, puis
//! l'échec, le `.part` gardé ; reprise après un arrêt pendant la vérification ou après
//! l'enregistrement d'un tour.

use std::sync::Mutex;

use sha1::{Digest, Sha1};

use super::*;
use crate::pieces::Pieces;
use crate::store::open_in_memory;
use crate::store::pieces::{load as load_pieces, start_round};
use crate::store::sources::{self, Checked};
use crate::store::tasks::{NewTask, create_task_with_pieces, ensure_queue, save_preparation};
use crate::testing::{FileSite, Pace, Server, content, content_range, serve_file_paced};

/// Morceaux de 64 Kio.
const PIECE: u64 = 64 * 1024;
const LEN: u64 = 600_000;

/// Une seule connexion par source : les plages demandées restent prévisibles.
fn opts() -> Options {
    Options {
        connections: Some(1),
        min_segment: 16 * 1024,
        buffer: 4096,
        checkpoint_every: Duration::from_millis(20),
        ..Options::default()
    }
}

fn client() -> Client {
    Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap()
}

fn server(pace: Option<Pace>) -> (Arc<Mutex<FileSite>>, Server) {
    let site = Arc::new(Mutex::new(FileSite::new(LEN)));
    let server = serve_file_paced(Arc::clone(&site), pace);
    (site, server)
}

/// Empreintes SHA-1 des morceaux du contenu servi.
fn pieces() -> Pieces {
    let hashes = (0..LEN.div_ceil(PIECE))
        .map(|i| {
            let start = i * PIECE;
            let len = PIECE.min(LEN - start) as usize;
            Sha1::digest(content_range(start, len))
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        })
        .collect();
    Pieces::new(checksum::Algo::Sha1, PIECE, hashes).unwrap()
}

/// Morceau qui contient l'octet `at`.
fn piece_of(at: u64) -> (u64, u64) {
    let start = at / PIECE * PIECE;
    (start, (start + PIECE).min(LEN) - 1)
}

struct Case {
    conn: Connection,
    task_id: String,
    dir: tempfile::TempDir,
}

impl Case {
    fn new(url: &str, checksum: Option<&Checksum>) -> Case {
        let mut conn = open_in_memory().unwrap();
        let queue = ensure_queue(&conn, "default").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (task_id, _) = create_task_with_pieces(
            &mut conn,
            &NewTask {
                queue_id: &queue,
                url,
                display_name: &display_name_for(url),
                dest_dir: dir.path().to_str().unwrap(),
                max_connections: None,
                speed_limit_bps: None,
                idempotency_key: None,
                requested_name: None,
                checksum,
                headers: &[],
                parent_id: None,
                public_only: false,
            },
            &[],
            Some(&pieces()),
            Default::default(),
        )
        .unwrap();
        Case { conn, task_id, dir }
    }

    async fn run(&mut self) -> Outcome {
        let (_, stop) = watch::channel(false);
        run(
            &mut self.conn,
            &client(),
            &self.task_id,
            &opts(),
            stop,
            &mut |_| {},
        )
        .await
        .unwrap()
    }

    fn task(&self) -> TaskRow {
        load_task(&self.conn, &self.task_id).unwrap()
    }

    /// Tours faits et morceaux retéléchargés.
    fn counts(&self) -> (u32, u64) {
        let p = load_pieces(&self.conn, &self.task_id).unwrap().unwrap();
        (p.rounds, p.redone)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Préparée à la main, son `.part` complet écrit avec l'octet `flipped` faux, tous ses
    /// segments terminés (deux, coupés à `cut`), en `verifying` : un arrêt pendant le calcul
    /// des empreintes.
    fn verifying_with_a_bad_byte(&mut self, flipped: u64, cut: u64) {
        let source = primary_source(&self.conn, &self.task_id).unwrap();
        let segments = [
            Segment {
                idx: 0,
                start: 0,
                end: cut - 1,
                written: cut,
                state: SegmentState::Done,
            },
            Segment {
                idx: 1,
                start: cut,
                end: LEN - 1,
                written: LEN - cut,
                state: SegmentState::Done,
            },
        ];
        save_preparation(
            &mut self.conn,
            &self.task_id,
            &Preparation {
                source_id: &source.id,
                accepts_ranges: true,
                etag: Some("\"v1\""),
                last_modified: None,
                total_bytes: Some(LEN),
                file_name: "f.bin",
            },
            &segments,
        )
        .unwrap();
        for s in &segments {
            store::segments::set_source(&self.conn, &self.task_id, s.idx, &source.id).unwrap();
        }
        let mut data = content(LEN as usize);
        data[flipped as usize] ^= 0xFF;
        std::fs::write(self.path("f.bin.part"), &data).unwrap();
        for state in [TaskState::Active, TaskState::Verifying] {
            change_state(&mut self.conn, &self.task_id, &Change::to(state)).unwrap();
        }
    }
}

#[tokio::test]
async fn only_the_bad_piece_is_downloaded_again() {
    let (site, server) = server(None);
    // L'octet 200 000 arrive faux une fois.
    site.lock().unwrap().corrupt = vec![200_000];
    site.lock().unwrap().corrupt_responses = 1;
    let mut case = Case::new(&server.url("/f.bin"), None);
    let outcome = case.run().await;
    assert_eq!(outcome, Outcome::Completed(case.path("f.bin")));
    assert_eq!(
        std::fs::read(case.path("f.bin")).unwrap(),
        content(LEN as usize)
    );
    // Le fichier entier, puis le seul morceau faux.
    assert_eq!(
        site.lock().unwrap().served,
        [(0, LEN - 1), piece_of(200_000)]
    );
    assert_eq!(case.counts(), (1, 1));
    let sources = load_sources(&case.conn, &case.task_id).unwrap();
    assert_eq!(sources[0].bad_pieces, 1);
    assert_eq!(case.task().state, TaskState::Completed);
}

#[tokio::test]
async fn pieces_still_bad_after_three_rounds_fail_and_keep_the_part() {
    let (site, server) = server(None);
    site.lock().unwrap().corrupt = vec![100, 400_000];
    site.lock().unwrap().corrupt_responses = usize::MAX;
    let mut case = Case::new(&server.url("/f.bin"), None);
    let outcome = case.run().await;
    let Outcome::Failed { code, detail } = &outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(*code, "CHECKSUM_MISMATCH");
    assert!(
        detail.contains("2 morceaux sur 10") && detail.ends_with("après 3 tours de reprise"),
        "{detail}"
    );
    // Le `.part` reste sous son nom provisoire.
    assert!(case.path("f.bin.part").is_file());
    assert!(!case.path("f.bin").exists());
    assert_eq!(case.counts(), (3, 6));
    // Le fichier entier, puis trois tours des deux morceaux.
    let served = site.lock().unwrap().served.clone();
    assert_eq!(served.len(), 1 + 3 * 2, "{served:?}");
    assert!(
        served[1..]
            .iter()
            .all(|r| *r == piece_of(100) || *r == piece_of(400_000))
    );

    // Le serveur réparé, l'utilisateur relance : trois tours de nouveau, et seuls les
    // morceaux faux repartent (le `.part` est complet).
    site.lock().unwrap().corrupt.clear();
    // Une remise en file automatique garde les tours (audit S4) ; la relance de
    // l'utilisateur les redonne.
    change_state(
        &mut case.conn,
        &case.task_id,
        &Change::to(TaskState::Queued),
    )
    .unwrap();
    assert_eq!(case.counts(), (3, 6));
    change_state(
        &mut case.conn,
        &case.task_id,
        &Change {
            pause_reason: Some(crate::store::tasks::PauseReason::User),
            ..Change::to(TaskState::Paused)
        },
    )
    .unwrap();
    crate::store::tasks::change_state_by_user(&mut case.conn, &case.task_id).unwrap();
    assert_eq!(case.counts(), (0, 6));
    let outcome = case.run().await;
    assert_eq!(outcome, Outcome::Completed(case.path("f.bin")));
    assert_eq!(
        std::fs::read(case.path("f.bin")).unwrap(),
        content(LEN as usize)
    );
    assert_eq!(site.lock().unwrap().served.len(), served.len() + 2);
    assert_eq!(case.counts(), (1, 8));
}

/// Les morceaux faux d'un miroir sont redemandés à la référence : un seul tour suffit.
#[tokio::test]
async fn bad_pieces_of_a_mirror_come_again_from_the_reference() {
    let pace = Some(Pace {
        chunk: 16 * 1024,
        delay: Duration::from_millis(20),
    });
    let (_r, reference) = server(pace);
    let (mirror_site, mirror) = server(pace);
    // Le miroir sert faux un octet sur 10 000, toujours.
    mirror_site.lock().unwrap().corrupt = (0..LEN).step_by(10_000).collect();
    mirror_site.lock().unwrap().corrupt_responses = usize::MAX;
    let mut case = Case::new(&reference.url("/f.bin"), None);
    let mirror_id =
        sources::add_source(&mut case.conn, &case.task_id, &mirror.url("/f.bin")).unwrap();
    let checked = Checked {
        accepts_ranges: true,
        etag: Some("\"v1\""),
        last_modified: None,
        content_length: Some(LEN),
    };
    sources::set_checked(&case.conn, &mirror_id, &checked).unwrap();

    let outcome = case.run().await;
    assert_eq!(outcome, Outcome::Completed(case.path("f.bin")));
    assert_eq!(
        std::fs::read(case.path("f.bin")).unwrap(),
        content(LEN as usize)
    );
    let sources = load_sources(&case.conn, &case.task_id).unwrap();
    let mirror = sources.iter().find(|s| s.id == mirror_id).unwrap();
    assert!(
        !mirror_site.lock().unwrap().served.is_empty(),
        "le miroir a servi"
    );
    assert!(mirror.bad_pieces >= 1, "{mirror:?}");
    if mirror.bad_pieces >= pieces::SET_ASIDE_AFTER {
        assert_eq!(mirror.state, SourceState::Failed, "écarté : {mirror:?}");
    }
    assert_eq!(sources[0].bad_pieces, 0, "la référence n'est pas en cause");
    assert_eq!(
        case.counts().0,
        1,
        "un seul tour : tout vient de la référence"
    );
}

/// Arrêt pendant le calcul des empreintes : la reprise vérifie, rouvre le morceau faux et
/// ne télécharge que lui.
#[tokio::test]
async fn a_stop_during_the_check_resumes_with_the_bad_piece_alone() {
    let (site, server) = server(None);
    let mut case = Case::new(&server.url("/f.bin"), None);
    case.verifying_with_a_bad_byte(330_000, 300_000);
    let outcome = case.run().await;
    assert_eq!(outcome, Outcome::Completed(case.path("f.bin")));
    assert_eq!(
        std::fs::read(case.path("f.bin")).unwrap(),
        content(LEN as usize)
    );
    // Morceau 5 (327 680 à 393 215) : à cheval sur rien, dans le second segment.
    assert_eq!(site.lock().unwrap().served, [piece_of(330_000)]);
    assert_eq!(case.counts(), (1, 1));
}

/// Arrêt juste après l'enregistrement d'un tour : la tâche est `active`, ses morceaux
/// rouverts ; une nouvelle exécution les télécharge, sans rien d'autre.
#[tokio::test]
async fn a_stop_after_a_round_is_saved_downloads_the_reopened_pieces() {
    let (site, server) = server(None);
    let mut case = Case::new(&server.url("/f.bin"), None);
    // Le morceau faux est à cheval sur les deux segments.
    let (start, end) = piece_of(300_000);
    case.verifying_with_a_bad_byte(start + 10, 300_000);
    start_round(
        &mut case.conn,
        &case.task_id,
        &[(start, end)],
        &[(start, end)],
    )
    .unwrap();
    assert_eq!(case.task().state, TaskState::Active);
    let outcome = case.run().await;
    assert_eq!(outcome, Outcome::Completed(case.path("f.bin")));
    assert_eq!(
        std::fs::read(case.path("f.bin")).unwrap(),
        content(LEN as usize)
    );
    let mut served = site.lock().unwrap().served.clone();
    served.sort_unstable();
    assert_eq!(served, [(start, 299_999), (300_000, end)]);
}

/// Avec l'empreinte du fichier : calculée dans la même lecture ; des morceaux justes et
/// une empreinte entière différente donnent l'échec habituel.
#[tokio::test]
async fn the_whole_checksum_is_read_with_the_pieces() {
    let (_site, server) = server(None);
    let good = Checksum::new(checksum::Algo::Sha256, &crate::testing::content_sha256(LEN)).unwrap();
    let mut case = Case::new(&server.url("/f.bin"), Some(&good));
    assert_eq!(case.run().await, Outcome::Completed(case.path("f.bin")));
    assert_eq!(case.counts(), (0, 0));

    let wrong = Checksum::new(checksum::Algo::Sha256, &"0".repeat(64)).unwrap();
    let mut case = Case::new(&server.url("/f.bin"), Some(&wrong));
    let outcome = case.run().await;
    assert!(
        matches!(&outcome, Outcome::Failed { code: "CHECKSUM_MISMATCH", detail } if detail.contains("SHA-256")),
        "{outcome:?}"
    );
    assert!(case.path("f.bin.part").is_file());
}

/// Un serveur sans plages ne peut pas renvoyer un morceau : échec tout de suite, le `.part`
/// gardé.
#[tokio::test]
async fn without_ranges_a_bad_piece_fails_at_once() {
    let (site, server) = server(None);
    site.lock().unwrap().ranges = false;
    site.lock().unwrap().corrupt = vec![5];
    site.lock().unwrap().corrupt_responses = usize::MAX;
    let mut case = Case::new(&server.url("/f.bin"), None);
    let outcome = case.run().await;
    assert!(
        matches!(&outcome, Outcome::Failed { code: "CHECKSUM_MISMATCH", detail } if detail.contains("0 tours")),
        "{outcome:?}"
    );
    assert!(case.path("f.bin.part").is_file());
    assert_eq!(case.counts(), (0, 0));
}
