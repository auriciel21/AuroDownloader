//! Empreintes de morceaux d'un fichier (F-12, SPEC §7.9, phase 7, étape 5) : celles
//! qu'un Metalink publie (`pieces`). Le fichier est découpé en morceaux de même longueur
//! (le dernier plus court) ; à la fin du téléchargement, une seule lecture du `.part` donne
//! l'empreinte de chacun, et seuls les morceaux faux se téléchargent de nouveau.
//!
//! Décisions de l'utilisateur (2026-11-17) : contrôle à la fin, en `verifying` ; un
//! morceau faux est redemandé de préférence à une autre source, et une source qui en a
//! envoyé [`SET_ASIDE_AFTER`] est écartée pour sa tâche ; [`MAX_ROUNDS`] tours, puis échec
//! `CHECKSUM_MISMATCH`, le `.part` gardé. Les torrents vérifient déjà leurs morceaux
//! (librqbit, SPEC §7.16) : ce module ne les concerne pas.

use crate::io::checksum::{Algo, Checksum};

/// Tours « morceaux faux retéléchargés, puis vérification » avant l'échec.
pub const MAX_ROUNDS: u32 = 3;
/// Morceaux faux envoyés par une source avant qu'elle soit écartée pour sa tâche.
pub const SET_ASIDE_AFTER: u32 = 2;
/// Morceaux au plus : un garde-fou (un Metalink de 4 Mio n'en décrit pas tant).
pub const MAX_PIECES: usize = 1 << 20;

/// Empreintes des morceaux d'un fichier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pieces {
    algo: Algo,
    length: u64,
    /// Une par morceau, dans l'ordre, en hexadécimal minuscule.
    hashes: Vec<String>,
}

impl Pieces {
    /// Morceaux de `length` octets ; chaque empreinte est vérifiée (longueur de
    /// l'algorithme, chiffres hexadécimaux) et mise en minuscules.
    pub fn new(algo: Algo, length: u64, hashes: Vec<String>) -> Result<Self, String> {
        if length == 0 {
            return Err("longueur de morceau nulle".into());
        }
        if hashes.is_empty() || hashes.len() > MAX_PIECES {
            return Err(format!("de 1 à {MAX_PIECES} morceaux"));
        }
        let hashes = hashes
            .iter()
            .map(|h| Checksum::new(algo, h).map(|c| c.value().to_owned()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(Pieces {
            algo,
            length,
            hashes,
        })
    }

    pub fn algo(&self) -> Algo {
        self.algo
    }

    /// Longueur de chaque morceau, sauf du dernier.
    pub fn length(&self) -> u64 {
        self.length
    }

    pub fn count(&self) -> usize {
        self.hashes.len()
    }

    pub fn hashes(&self) -> &[String] {
        &self.hashes
    }

    /// Le nombre de morceaux convient à un fichier de `total` octets.
    pub fn fits(&self, total: u64) -> bool {
        total > 0 && total.div_ceil(self.length) == self.count() as u64
    }

    /// Empreintes mises bout à bout, pour la colonne `task_pieces.hashes`.
    pub fn to_sql(&self) -> String {
        self.hashes.concat()
    }

    /// Relit la colonne `task_pieces.hashes`.
    pub fn from_sql(algo: Algo, length: u64, text: &str) -> Result<Self, String> {
        let width = algo.hex_len();
        if text.is_empty() || !text.len().is_multiple_of(width) || !text.is_ascii() {
            return Err("empreintes de morceaux mal enregistrées".into());
        }
        let hashes = (0..text.len() / width)
            .map(|i| text[i * width..(i + 1) * width].to_owned())
            .collect();
        Self::new(algo, length, hashes)
    }

    /// Premier et dernier octet (inclus) du morceau `i` d'un fichier de `total` octets.
    pub fn range(&self, i: usize, total: u64) -> (u64, u64) {
        let start = i as u64 * self.length;
        (start, (start + self.length).min(total) - 1)
    }

    /// Rangs des morceaux dont l'empreinte obtenue (`got`, une par morceau) diffère.
    /// Un morceau sans empreinte obtenue compte pour faux.
    pub fn bad(&self, got: &[String]) -> Vec<usize> {
        (0..self.count())
            .filter(|&i| got.get(i).is_none_or(|g| *g != self.hashes[i]))
            .collect()
    }

    /// Plages d'octets des morceaux `bad` (rangs croissants), les voisins réunis.
    pub fn ranges(&self, bad: &[usize], total: u64) -> Vec<(u64, u64)> {
        let mut out: Vec<(u64, u64)> = Vec::new();
        for &i in bad {
            let (start, end) = self.range(i, total);
            match out.last_mut() {
                Some(last) if last.1 + 1 == start => last.1 = end,
                _ => out.push((start, end)),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(c: char, algo: Algo) -> String {
        std::iter::repeat_n(c, algo.hex_len()).collect()
    }

    #[test]
    fn values_are_checked_and_kept_in_lowercase() {
        let p = Pieces::new(Algo::Sha1, 4, vec![hex('A', Algo::Sha1)]).unwrap();
        assert_eq!(p.hashes(), [hex('a', Algo::Sha1)]);
        assert!(Pieces::new(Algo::Sha1, 0, vec![hex('a', Algo::Sha1)]).is_err());
        assert!(Pieces::new(Algo::Sha1, 4, vec![]).is_err());
        assert!(Pieces::new(Algo::Sha256, 4, vec![hex('a', Algo::Sha1)]).is_err());
        assert!(Pieces::new(Algo::Md5, 4, vec![hex('g', Algo::Md5)]).is_err());
    }

    #[test]
    fn pieces_cover_the_file_and_the_last_one_is_shorter() {
        let p = Pieces::new(Algo::Md5, 10, vec![hex('0', Algo::Md5); 3]).unwrap();
        assert!(p.fits(21) && p.fits(30));
        assert!(!p.fits(20) && !p.fits(31) && !p.fits(0));
        assert_eq!(p.range(0, 25), (0, 9));
        assert_eq!(p.range(2, 25), (20, 24));
    }

    #[test]
    fn bad_pieces_become_merged_ranges() {
        let good = |c| hex(c, Algo::Md5);
        let p = Pieces::new(
            Algo::Md5,
            10,
            vec![good('0'), good('1'), good('2'), good('3')],
        )
        .unwrap();
        let got = vec![good('0'), good('f'), good('f'), good('3')];
        assert_eq!(p.bad(&got), [1, 2]);
        assert_eq!(p.ranges(&[1, 2], 35), [(10, 29)]);
        assert_eq!(p.ranges(&[0, 3], 35), [(0, 9), (30, 34)]);
        // Lecture interrompue : les morceaux sans empreinte comptent pour faux.
        assert_eq!(p.bad(&got[..1]), [1, 2, 3]);
    }

    #[test]
    fn the_column_reads_back() {
        let p = Pieces::new(
            Algo::Sha1,
            7,
            vec![hex('a', Algo::Sha1), hex('b', Algo::Sha1)],
        )
        .unwrap();
        assert_eq!(Pieces::from_sql(Algo::Sha1, 7, &p.to_sql()).unwrap(), p);
        assert!(Pieces::from_sql(Algo::Sha1, 7, &p.to_sql()[1..]).is_err());
        assert!(Pieces::from_sql(Algo::Sha1, 7, "").is_err());
    }
}
