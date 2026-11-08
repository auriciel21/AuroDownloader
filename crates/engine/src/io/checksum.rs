//! Empreinte du fichier complet (F-11, SPEC §7.9) : SHA-256, SHA-1 ou MD5, calculée par
//! une lecture séquentielle du `.part` vidé sur disque, avant son renommage.

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256};

/// Algorithme d'une empreinte ; les valeurs de la colonne `tasks.checksum_algo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algo {
    Sha256,
    Sha1,
    Md5,
}

impl Algo {
    pub fn as_sql(self) -> &'static str {
        match self {
            Algo::Sha256 => "sha256",
            Algo::Sha1 => "sha1",
            Algo::Md5 => "md5",
        }
    }

    pub fn from_sql(s: &str) -> Option<Self> {
        match s {
            "sha256" => Some(Algo::Sha256),
            "sha1" => Some(Algo::Sha1),
            "md5" => Some(Algo::Md5),
            _ => None,
        }
    }

    /// Nom affiché, dans les messages.
    pub fn label(self) -> &'static str {
        match self {
            Algo::Sha256 => "SHA-256",
            Algo::Sha1 => "SHA-1",
            Algo::Md5 => "MD5",
        }
    }

    /// Longueur de l'empreinte en chiffres hexadécimaux.
    pub fn hex_len(self) -> usize {
        match self {
            Algo::Sha256 => 64,
            Algo::Sha1 => 40,
            Algo::Md5 => 32,
        }
    }
}

/// Empreinte attendue d'un fichier : algorithme et valeur en hexadécimal minuscule.
#[derive(Clone, PartialEq, Eq)]
pub struct Checksum {
    algo: Algo,
    value: String,
}

/// Empreinte refusée : algorithme inconnu ou valeur mal formée.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChecksumError {
    #[error("algorithme d'empreinte inconnu (sha256, sha1 ou md5)")]
    UnknownAlgo,
    #[error("une empreinte {algo} s'écrit en {len} chiffres hexadécimaux")]
    BadValue { algo: &'static str, len: usize },
    #[error(
        "empreinte de 64 (SHA-256), 40 (SHA-1) ou 32 (MD5) chiffres hexadécimaux attendue, \
         ou « sha256:… »"
    )]
    UnknownLength,
}

impl Checksum {
    /// Empreinte `algo` (`sha256`, `sha1`, `md5`, sans égard à la casse) de valeur `value`
    /// en hexadécimal, majuscules acceptées ; les espaces autour sont ignorés.
    pub fn parse(algo: &str, value: &str) -> Result<Self, ChecksumError> {
        let algo = Algo::from_sql(&algo.trim().to_ascii_lowercase().replace('-', ""))
            .ok_or(ChecksumError::UnknownAlgo)?;
        Self::new(algo, value)
    }

    /// Empreinte écrite par l'utilisateur : « sha256:… » (ou `sha1:`, `md5:`), ou la
    /// valeur seule, dont la longueur donne l'algorithme.
    pub fn from_text(text: &str) -> Result<Self, ChecksumError> {
        let text = text.trim();
        if let Some((algo, value)) = text.split_once(':') {
            return Self::parse(algo, value);
        }
        let algo = [Algo::Sha256, Algo::Sha1, Algo::Md5]
            .into_iter()
            .find(|a| a.hex_len() == text.len())
            .ok_or(ChecksumError::UnknownLength)?;
        Self::new(algo, text)
    }

    pub fn new(algo: Algo, value: &str) -> Result<Self, ChecksumError> {
        let value = value.trim();
        if value.len() != algo.hex_len() || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ChecksumError::BadValue {
                algo: algo.label(),
                len: algo.hex_len(),
            });
        }
        Ok(Checksum {
            algo,
            value: value.to_ascii_lowercase(),
        })
    }

    pub fn algo(&self) -> Algo {
        self.algo
    }

    /// Valeur en hexadécimal minuscule.
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Debug for Checksum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algo.as_sql(), self.value)
    }
}

/// Taille des lectures du calcul.
const READ_SIZE: usize = 1024 * 1024;

/// Empreinte `algo` du fichier `path`, en hexadécimal minuscule. `on_read` reçoit les
/// octets lus jusque-là, après chaque lecture, et arrête le calcul en renvoyant `false` :
/// le résultat est alors `Ok(None)`.
pub fn hash_file(
    path: &Path,
    algo: Algo,
    mut on_read: impl FnMut(u64) -> bool,
) -> io::Result<Option<String>> {
    let mut file = File::open(path)?;
    let mut hasher: Box<dyn DynHasher> = match algo {
        Algo::Sha256 => Box::new(Sha256::new()),
        Algo::Sha1 => Box::new(Sha1::new()),
        Algo::Md5 => Box::new(Md5::new()),
    };
    let mut buf = vec![0u8; READ_SIZE];
    let mut done = 0u64;
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.feed(&buf[..n]);
        done += n as u64;
        if !on_read(done) {
            return Ok(None);
        }
    }
    Ok(Some(hasher.hex()))
}

/// Les trois algorithmes derrière une même interface.
trait DynHasher {
    fn feed(&mut self, data: &[u8]);
    fn hex(self: Box<Self>) -> String;
}

impl<D: Digest> DynHasher for D {
    fn feed(&mut self, data: &[u8]) {
        Digest::update(self, data);
    }

    fn hex(self: Box<Self>) -> String {
        use fmt::Write;
        let digest = (*self).finalize();
        let mut out = String::with_capacity(digest.len() * 2);
        for b in digest {
            let _ = write!(out, "{b:02x}");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_checked_and_normalised() {
        let sum = Checksum::parse(
            "SHA-256",
            " E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855 ",
        )
        .unwrap();
        assert_eq!(sum.algo(), Algo::Sha256);
        assert_eq!(
            sum.value(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            Checksum::parse("crc32", "00000000"),
            Err(ChecksumError::UnknownAlgo)
        );
        // Longueur d'un autre algorithme, ou caractère qui n'est pas hexadécimal.
        assert!(Checksum::parse("md5", "da39a3ee5e6b4b0d3255bfef95601890afd80709").is_err());
        assert!(Checksum::parse("sha1", "da39a3ee5e6b4b0d3255bfef95601890afd8070g").is_err());
        assert!(Checksum::parse("sha1", "da39a3ee5e6b4b0d3255bfef95601890afd80709").is_ok());
    }

    #[test]
    fn typed_values_name_their_algorithm_or_give_it_by_length() {
        let sha1 = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
        let md5 = "d41d8cd98f00b204e9800998ecf8427e";
        let sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        for (text, algo) in [
            (sha256, Algo::Sha256),
            (sha1, Algo::Sha1),
            (md5, Algo::Md5),
            (&format!("SHA1:{sha1}"), Algo::Sha1),
            (&format!(" md5:{md5} "), Algo::Md5),
        ] {
            assert_eq!(Checksum::from_text(text).unwrap().algo(), algo, "{text}");
        }
        assert_eq!(
            Checksum::from_text("abcdef"),
            Err(ChecksumError::UnknownLength)
        );
        // L'algorithme nommé doit avoir la bonne longueur.
        assert!(Checksum::from_text(&format!("sha256:{sha1}")).is_err());
    }

    #[test]
    fn known_digests_of_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abc");
        std::fs::write(&path, b"abc").unwrap();
        let hash = |algo| hash_file(&path, algo, |_| true).unwrap().unwrap();
        // Valeurs de référence : FIPS 180-4 (SHA-256, SHA-1) et RFC 1321 (MD5).
        assert_eq!(
            hash(Algo::Sha256),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(hash(Algo::Sha1), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hash(Algo::Md5), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn a_long_file_reports_progress_and_can_be_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long");
        let data: Vec<u8> = (0..3 * READ_SIZE + 17).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        let mut seen = Vec::new();
        let full = hash_file(&path, Algo::Sha256, |n| {
            seen.push(n);
            true
        })
        .unwrap()
        .unwrap();
        assert_eq!(seen.last(), Some(&(data.len() as u64)));
        assert!(seen.windows(2).all(|w| w[0] < w[1]));
        let expected: String = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(full, expected);
        // Arrêt demandé après la première lecture.
        assert_eq!(hash_file(&path, Algo::Sha256, |_| false).unwrap(), None);
    }
}
