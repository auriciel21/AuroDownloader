//! Découpage et redécoupage des segments, répartition sur les sources, nombre de
//! connexions. Voir SPEC §7.2, §7.3 et §7.4.
//!
//! Ce module ne fait ni réseau ni disque : il décide des plages d'octets. L'invariant
//! central (SPEC §5.6, règle 2) est vérifié par [`check_coverage`] : les segments d'une
//! tâche couvrent `[0, total - 1]` sans trou ni chevauchement, y compris après un nombre
//! quelconque de redécoupages ([`split_largest`]).
//!
//! Une tâche de taille nulle n'a aucun segment ; une tâche de taille inconnue est
//! téléchargée en un seul flux, sans segments, et n'est pas concernée.

use std::cmp::Reverse;
use std::collections::HashSet;

/// Taille minimale d'un segment par défaut : 1 Mio (SPEC §7.10).
pub const DEFAULT_MIN_SEGMENT_SIZE: u64 = 1024 * 1024;

/// État d'un segment, comme la colonne `segments.state` (SPEC §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentState {
    Pending,
    Active,
    Done,
    Failed,
}

/// Plage d'octets d'un fichier, téléchargée par une connexion (SPEC §5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Numéro du segment dans sa tâche : croît à chaque redécoupage, jamais réutilisé.
    pub idx: u32,
    /// Premier octet de la plage.
    pub start: u64,
    /// Dernier octet de la plage, inclus.
    pub end: u64,
    /// Octets écrits et vidés sur disque, à partir de `start`.
    pub written: u64,
    pub state: SegmentState,
}

impl Segment {
    /// Nombre d'octets de la plage (suppose `end >= start`, voir [`check_coverage`]).
    pub fn size(&self) -> u64 {
        self.end - self.start + 1
    }

    /// Octets qui restent à télécharger.
    pub fn remaining(&self) -> u64 {
        self.size().saturating_sub(self.written)
    }

    /// Premier octet à demander : `Range: bytes=(start + written)-(end)` (SPEC §7.2).
    pub fn next_offset(&self) -> u64 {
        self.start + self.written
    }
}

/// Violation de l'invariant de couverture (SPEC §5.6, règle 2).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoverageError {
    #[error("segment {idx} invalide : plage {start}-{end}, {written} octets écrits")]
    InvalidSegment {
        idx: u32,
        start: u64,
        end: u64,
        written: u64,
    },

    #[error("le numéro de segment {idx} est utilisé deux fois")]
    DuplicateIndex { idx: u32 },

    #[error("le segment {idx} finit à l'octet {end}, au-delà d'un fichier de {total} octets")]
    BeyondEnd { idx: u32, end: u64, total: u64 },

    #[error("aucun segment ne couvre les octets {start} à {end}")]
    Gap { start: u64, end: u64 },

    #[error("les segments {first} et {second} se chevauchent à partir de l'octet {at}")]
    Overlap { first: u32, second: u32, at: u64 },
}

/// Vérifie que `segments` couvrent exactement `[0, total - 1]`, sans trou ni
/// chevauchement, et que chaque segment est cohérent (SPEC §5.6, règle 2).
///
/// Pour `total == 0`, seule une liste vide est acceptée.
pub fn check_coverage(total: u64, segments: &[Segment]) -> Result<(), CoverageError> {
    let mut seen = HashSet::with_capacity(segments.len());
    for s in segments {
        let invalid = || CoverageError::InvalidSegment {
            idx: s.idx,
            start: s.start,
            end: s.end,
            written: s.written,
        };
        if s.end < s.start {
            return Err(invalid());
        }
        if s.end >= total {
            return Err(CoverageError::BeyondEnd {
                idx: s.idx,
                end: s.end,
                total,
            });
        }
        // `end < total` : le calcul de `size` ne peut plus déborder.
        if s.written > s.size() {
            return Err(invalid());
        }
        if !seen.insert(s.idx) {
            return Err(CoverageError::DuplicateIndex { idx: s.idx });
        }
    }

    let mut sorted: Vec<&Segment> = segments.iter().collect();
    sorted.sort_by_key(|s| (s.start, s.idx));

    // Premier octet pas encore couvert, et segment qui couvre l'octet précédent.
    let mut expected = 0;
    let mut previous_idx = 0;
    for s in sorted {
        if s.start > expected {
            return Err(CoverageError::Gap {
                start: expected,
                end: s.start - 1,
            });
        }
        if s.start < expected {
            return Err(CoverageError::Overlap {
                first: previous_idx,
                second: s.idx,
                at: s.start,
            });
        }
        expected = s.end + 1;
        previous_idx = s.idx;
    }
    if expected < total {
        return Err(CoverageError::Gap {
            start: expected,
            end: total - 1,
        });
    }
    Ok(())
}

/// Découpage initial de `[0, total - 1]` en `N = min(connexions, total / taille minimale)`
/// segments `pending` de tailles égales à un octet près (SPEC §7.2).
///
/// Un fichier plus petit que la taille minimale donne un seul segment ; un fichier vide,
/// aucun.
pub fn initial_split(total: u64, connections: u32, min_size: u64) -> Vec<Segment> {
    if total == 0 {
        return Vec::new();
    }
    let connections = connections.max(1);
    let by_size = total / min_size.max(1);
    let count = u32::try_from(by_size)
        .map_or(connections, |n| n.min(connections))
        .max(1);

    let base = total / u64::from(count);
    // Les `extra` premiers segments prennent un octet de plus.
    let extra = total % u64::from(count);
    let mut segments = Vec::new();
    let mut start = 0;
    for idx in 0..count {
        let size = base + u64::from(u64::from(idx) < extra);
        segments.push(Segment {
            idx,
            start,
            end: start + size - 1,
            written: 0,
            state: SegmentState::Pending,
        });
        start += size;
    }
    segments
}

/// Résultat d'un redécoupage : le segment `from` a été raccourci et `new` couvre la fin
/// qu'il a cédée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Split {
    pub from: u32,
    pub new: u32,
}

/// Coupe en deux le segment `active` qui a le plus d'octets restants (SPEC §7.2, F-02).
///
/// À appeler quand une connexion se libère et qu'aucun segment `pending` n'est disponible.
/// Le nouveau segment, `active`, prend la seconde moitié de ce qui reste ; les octets déjà
/// écrits restent dans le segment d'origine. Renvoie `None` si aucun segment actif n'a au
/// moins deux fois `min_size` octets restants (fin de tâche proche).
///
/// L'appelant arrête la connexion du segment `from` à sa nouvelle fin, et enregistre les
/// deux segments dans la même transaction que la vérification de [`check_coverage`].
pub fn split_largest(segments: &mut Vec<Segment>, min_size: u64) -> Option<Split> {
    // `min_size >= 1` garantit que le milieu tombe après `next_offset`, donc que le
    // segment raccourci garde au moins un octet à télécharger.
    let min_size = min_size.max(1);
    let new_idx = segments.iter().map(|s| s.idx).max()?.checked_add(1)?;

    let largest = segments
        .iter_mut()
        .filter(|s| s.state == SegmentState::Active)
        .max_by_key(|s| (s.remaining(), Reverse(s.idx)))?;
    let remaining = largest.remaining();
    if remaining < min_size.saturating_mul(2) {
        return None;
    }
    let middle = largest.next_offset() + remaining / 2;
    let new = Segment {
        idx: new_idx,
        start: middle,
        end: largest.end,
        written: 0,
        state: SegmentState::Active,
    };
    largest.end = middle - 1;
    let from = largest.idx;
    segments.push(new);
    Some(Split { from, new: new_idx })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const MIB: u64 = 1024 * 1024;

    fn seg(idx: u32, start: u64, end: u64, written: u64, state: SegmentState) -> Segment {
        Segment {
            idx,
            start,
            end,
            written,
            state,
        }
    }

    fn ranges(segments: &[Segment]) -> Vec<(u64, u64)> {
        segments.iter().map(|s| (s.start, s.end)).collect()
    }

    #[test]
    fn empty_file_has_no_segments() {
        assert!(initial_split(0, 8, MIB).is_empty());
        assert_eq!(check_coverage(0, &[]), Ok(()));
        assert!(matches!(
            check_coverage(0, &[seg(0, 0, 0, 0, SegmentState::Pending)]),
            Err(CoverageError::BeyondEnd { .. })
        ));
    }

    #[test]
    fn file_smaller_than_min_size_gets_one_segment() {
        let segments = initial_split(1000, 8, MIB);
        assert_eq!(ranges(&segments), [(0, 999)]);
        assert_eq!(check_coverage(1000, &segments), Ok(()));
    }

    #[test]
    fn initial_split_is_limited_by_min_size() {
        let segments = initial_split(10 * MIB, 32, MIB);
        assert_eq!(segments.len(), 10);
        assert!(segments.iter().all(|s| s.size() == MIB));
    }

    #[test]
    fn initial_split_gives_the_remainder_to_the_first_segments() {
        let segments = initial_split(10, 3, 1);
        assert_eq!(ranges(&segments), [(0, 3), (4, 6), (7, 9)]);
        assert!(segments.iter().all(|s| s.state == SegmentState::Pending));
    }

    #[test]
    fn zero_connections_or_zero_min_size_still_split() {
        assert_eq!(initial_split(100, 0, 10).len(), 1);
        assert_eq!(initial_split(100, 4, 0).len(), 4);
    }

    #[test]
    fn gap_is_detected_in_the_middle_and_at_the_end() {
        let middle = [
            seg(0, 0, 9, 0, SegmentState::Pending),
            seg(1, 20, 29, 0, SegmentState::Pending),
        ];
        assert_eq!(
            check_coverage(30, &middle),
            Err(CoverageError::Gap { start: 10, end: 19 })
        );
        let end = [seg(0, 0, 9, 0, SegmentState::Pending)];
        assert_eq!(
            check_coverage(30, &end),
            Err(CoverageError::Gap { start: 10, end: 29 })
        );
        assert_eq!(
            check_coverage(30, &[]),
            Err(CoverageError::Gap { start: 0, end: 29 })
        );
    }

    #[test]
    fn overlap_is_detected_whatever_the_order() {
        let segments = [
            seg(7, 10, 29, 0, SegmentState::Pending),
            seg(3, 0, 10, 0, SegmentState::Pending),
        ];
        assert_eq!(
            check_coverage(30, &segments),
            Err(CoverageError::Overlap {
                first: 3,
                second: 7,
                at: 10
            })
        );
    }

    #[test]
    fn inconsistent_segments_are_rejected() {
        let reversed = [seg(0, 10, 5, 0, SegmentState::Pending)];
        assert!(matches!(
            check_coverage(30, &reversed),
            Err(CoverageError::InvalidSegment { .. })
        ));
        let overwritten = [seg(0, 0, 9, 11, SegmentState::Active)];
        assert!(matches!(
            check_coverage(10, &overwritten),
            Err(CoverageError::InvalidSegment { .. })
        ));
        let duplicate = [
            seg(0, 0, 4, 0, SegmentState::Pending),
            seg(0, 5, 9, 0, SegmentState::Pending),
        ];
        assert_eq!(
            check_coverage(10, &duplicate),
            Err(CoverageError::DuplicateIndex { idx: 0 })
        );
        let huge = [seg(0, 0, u64::MAX, 0, SegmentState::Pending)];
        assert!(matches!(
            check_coverage(u64::MAX, &huge),
            Err(CoverageError::BeyondEnd { .. })
        ));
    }

    #[test]
    fn split_cuts_the_remaining_bytes_in_half() {
        // 100 octets, 10 déjà écrits : il en reste 90, le milieu tombe à 10 + 45.
        let mut segments = vec![seg(0, 0, 99, 10, SegmentState::Active)];
        assert_eq!(
            split_largest(&mut segments, 10),
            Some(Split { from: 0, new: 1 })
        );
        assert_eq!(ranges(&segments), [(0, 54), (55, 99)]);
        assert_eq!(segments[0].written, 10);
        assert_eq!(segments[1].written, 0);
        assert_eq!(segments[1].state, SegmentState::Active);
        assert_eq!(check_coverage(100, &segments), Ok(()));
    }

    #[test]
    fn split_is_refused_below_twice_the_min_size() {
        let mut segments = vec![seg(0, 0, 99, 81, SegmentState::Active)];
        assert_eq!(split_largest(&mut segments, 10), None);
        segments[0].written = 80;
        assert!(split_largest(&mut segments, 10).is_some());
    }

    #[test]
    fn split_takes_the_active_segment_with_most_remaining_bytes() {
        let mut segments = vec![
            seg(0, 0, 99, 0, SegmentState::Pending),
            seg(1, 100, 199, 90, SegmentState::Active),
            seg(2, 200, 299, 20, SegmentState::Active),
            seg(3, 300, 399, 0, SegmentState::Done),
        ];
        assert_eq!(
            split_largest(&mut segments, 1),
            Some(Split { from: 2, new: 4 })
        );
        assert_eq!(segments[2].end, 259);
        assert_eq!((segments[4].start, segments[4].end), (260, 299));
    }

    #[test]
    fn split_never_reuses_an_index() {
        let mut segments = vec![
            seg(5, 0, 999, 0, SegmentState::Active),
            seg(2, 1000, 1999, 1000, SegmentState::Done),
        ];
        let first = split_largest(&mut segments, 1).unwrap();
        let second = split_largest(&mut segments, 1).unwrap();
        assert_eq!((first.new, second.new), (6, 7));
    }

    #[test]
    fn split_without_active_segment_does_nothing() {
        let mut segments = initial_split(100 * MIB, 4, MIB);
        let before = segments.clone();
        assert_eq!(split_largest(&mut segments, MIB), None);
        assert_eq!(segments, before);
        assert_eq!(split_largest(&mut Vec::new(), MIB), None);
    }

    /// Ce qui peut arriver à un segment pendant un téléchargement.
    #[derive(Debug, Clone)]
    enum Op {
        /// Une connexion prend un segment `pending`.
        Activate(usize),
        /// Une connexion écrit des octets dans un segment `active`.
        Write(usize, u64),
        /// Un segment `active` échoue, puis revient à `pending` (SPEC §7.4).
        Fail(usize),
        /// Une connexion libre redécoupe le plus gros segment.
        Split,
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            2 => any::<usize>().prop_map(Op::Activate),
            3 => (any::<usize>(), 0..64 * MIB).prop_map(|(i, n)| Op::Write(i, n)),
            1 => any::<usize>().prop_map(Op::Fail),
            4 => Just(Op::Split),
        ]
    }

    fn total() -> impl Strategy<Value = u64> {
        prop_oneof![0..64u64, 0..(1u64 << 36)]
    }

    fn min_size() -> impl Strategy<Value = u64> {
        prop_oneof![1..8u64, 1..=4 * MIB]
    }

    fn apply(segments: &mut Vec<Segment>, op: &Op, min_size: u64) {
        let len = segments.len();
        let pick = |i: usize| (len > 0).then(|| i % len);
        match *op {
            Op::Activate(i) => {
                if let Some(s) = pick(i).map(|k| &mut segments[k]) {
                    if matches!(s.state, SegmentState::Pending | SegmentState::Failed) {
                        s.state = SegmentState::Active;
                    }
                }
            }
            Op::Write(i, n) => {
                if let Some(s) = pick(i).map(|k| &mut segments[k]) {
                    if s.state == SegmentState::Active {
                        s.written += n.min(s.remaining());
                        if s.remaining() == 0 {
                            s.state = SegmentState::Done;
                        }
                    }
                }
            }
            Op::Fail(i) => {
                if let Some(s) = pick(i).map(|k| &mut segments[k]) {
                    if s.state == SegmentState::Active {
                        s.state = SegmentState::Failed;
                    }
                }
            }
            Op::Split => {
                split_largest(segments, min_size);
            }
        }
    }

    proptest! {
        #[test]
        fn initial_split_covers_the_file(
            total in total(),
            connections in 0..=32u32,
            min_size in min_size(),
        ) {
            let segments = initial_split(total, connections, min_size);
            prop_assert_eq!(check_coverage(total, &segments), Ok(()));
            prop_assert!(segments.len() <= connections.max(1) as usize);
            if segments.len() > 1 {
                prop_assert!(segments.iter().all(|s| s.size() >= min_size));
            }
            let sizes = segments.iter().map(Segment::size);
            if let (Some(lo), Some(hi)) = (sizes.clone().min(), sizes.max()) {
                prop_assert!(hi - lo <= 1, "tailles inégales : {lo} et {hi}");
            }
        }

        #[test]
        fn coverage_holds_after_random_splits(
            total in total(),
            connections in 1..=32u32,
            min_size in min_size(),
            ops in prop::collection::vec(op(), 0..300),
        ) {
            let mut segments = initial_split(total, connections, min_size);
            for op in &ops {
                let before = segments.clone();
                apply(&mut segments, op, min_size);
                prop_assert_eq!(check_coverage(total, &segments), Ok(()), "après {:?}", op);

                if segments.len() > before.len() {
                    // Un redécoupage ne perd aucun octet écrit et ne déplace aucun début.
                    for (old, now) in before.iter().zip(&segments) {
                        prop_assert_eq!((old.idx, old.start, old.written), (now.idx, now.start, now.written));
                    }
                    let new = &segments[before.len()];
                    prop_assert_eq!(new.written, 0);
                    prop_assert!(new.idx > before.iter().map(|s| s.idx).max().unwrap_or(0));
                    prop_assert!(new.size() >= min_size);
                }
            }
        }

        #[test]
        fn removing_or_stretching_a_segment_is_detected(
            total in 1..(1u64 << 36),
            connections in 1..=32u32,
            min_size in min_size(),
            ops in prop::collection::vec(op(), 0..100),
            pick in any::<usize>(),
        ) {
            let mut segments = initial_split(total, connections, min_size);
            for op in &ops {
                apply(&mut segments, op, min_size);
            }
            let k = pick % segments.len();

            let mut removed = segments.clone();
            removed.remove(k);
            let is_gap = matches!(check_coverage(total, &removed), Err(CoverageError::Gap { .. }));
            prop_assert!(is_gap);

            let mut stretched = segments.clone();
            stretched[k].end += 1;
            let is_detected = matches!(
                check_coverage(total, &stretched),
                Err(CoverageError::Overlap { .. } | CoverageError::BeyondEnd { .. })
            );
            prop_assert!(is_detected);
        }
    }
}
