//! Empreinte lue dans un fichier voisin (F-11, SPEC §7.9) : un fichier compagnon
//! (`image.iso.sha256`) à côté du fichier, ou une liste (`SHA256SUMS`) dans son dossier.
//!
//! Les requêtes partent sans les en-têtes ni les identifiants de la tâche : le fichier
//! voisin est public quand il est utile, et un cookie n'a pas à suivre une adresse que
//! la tâche n'a pas demandée.

use std::time::Duration;

use reqwest::Url;
use tokio::task::JoinSet;

use super::{Client, filename};
use crate::io::checksum::{Algo, Checksum};

/// Taille maximale d'un fichier voisin : une liste d'empreintes tient en quelques Kio.
pub const MAX_LEN: usize = 1 << 20;

/// Durée maximale de la recherche, toutes requêtes comprises.
pub const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Fichier compagnon : le nom du fichier suivi de cette extension.
    Companion(&'static str),
    /// Liste d'empreintes du dossier.
    List(&'static str),
}

/// Fichiers voisins essayés, du préféré au moins bon : SHA-256 d'abord.
const CANDIDATES: [(Algo, Kind); 6] = [
    (Algo::Sha256, Kind::Companion("sha256")),
    (Algo::Sha256, Kind::List("SHA256SUMS")),
    (Algo::Sha1, Kind::Companion("sha1")),
    (Algo::Sha1, Kind::List("SHA1SUMS")),
    (Algo::Md5, Kind::Companion("md5")),
    (Algo::Md5, Kind::List("MD5SUMS")),
];

/// Adresse d'un fichier voisin de `url`, sans sa requête ni son fragment (une signature
/// d'adresse ne vaut que pour le fichier lui-même), et son nom, qui sert d'origine.
fn neighbor(url: &Url, name: &str, kind: Kind) -> Option<(Url, String)> {
    let mut base = url.clone();
    base.set_query(None);
    base.set_fragment(None);
    if base.path().ends_with('/') {
        return None;
    }
    match kind {
        Kind::Companion(ext) => {
            let path = format!("{}.{ext}", base.path());
            base.set_path(&path);
            Some((base, format!("{name}.{ext}")))
        }
        Kind::List(list) => Some((base.join(list).ok()?, list.to_owned())),
    }
}

/// Étiquette d'une ligne au format BSD (`SHA256 (nom) = empreinte`).
fn bsd_tag(algo: Algo) -> &'static str {
    match algo {
        Algo::Sha256 => "SHA256",
        Algo::Sha1 => "SHA1",
        Algo::Md5 => "MD5",
    }
}

/// Une ligne d'empreinte : la valeur, et le nom qu'elle désigne s'il y en a un.
/// Formats : `empreinte  nom` et `empreinte *nom` (GNU), `SHA256 (nom) = empreinte`
/// (BSD), ou l'empreinte seule. Une ligne GNU échappée (`\` au début) est ignorée.
fn entry(line: &str, algo: Algo) -> Option<(&str, Option<&str>)> {
    if let Some(rest) = line
        .strip_prefix(bsd_tag(algo))
        .and_then(|r| r.strip_prefix(" ("))
    {
        let (name, hex) = rest.rsplit_once(") = ")?;
        return Some((hex.trim(), Some(name)));
    }
    let end = line
        .find(|c: char| !c.is_ascii_hexdigit())
        .unwrap_or(line.len());
    let (hex, rest) = line.split_at(end);
    if hex.is_empty() {
        return None;
    }
    if rest.trim().is_empty() {
        return Some((hex, None));
    }
    let rest = rest.strip_prefix(' ')?;
    let name = rest.strip_prefix(['*', ' ']).unwrap_or(rest);
    Some((hex, Some(name)))
}

/// Empreinte `algo` du fichier appelé `names` (l'un ou l'autre) dans le contenu d'un
/// fichier voisin. Une ligne compte si elle nomme exactement le fichier (`./` admis) ;
/// une empreinte seule seulement si `alone` (fichier compagnon, qui ne vaut que pour
/// lui). `None` si rien ne convient, ou si deux lignes donnent deux empreintes.
pub fn find_in(text: &str, algo: Algo, names: &[&str], alone: bool) -> Option<Checksum> {
    let mut found: Option<Checksum> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((hex, name)) = entry(line, algo) else {
            continue;
        };
        let wanted = match name {
            None => alone,
            Some(name) => names.contains(&name.strip_prefix("./").unwrap_or(name)),
        };
        if !wanted {
            continue;
        }
        let Ok(checksum) = Checksum::new(algo, hex) else {
            continue;
        };
        match &found {
            Some(earlier) if earlier.value() != checksum.value() => return None,
            _ => found = Some(checksum),
        }
    }
    found
}

/// Cherche l'empreinte du fichier `url` dans ses fichiers voisins, toutes les requêtes
/// en même temps, pendant au plus [`TIMEOUT`]. `names` : d'autres noms du fichier que
/// celui de l'adresse (celui de `Content-Disposition`). Renvoie la meilleure empreinte
/// trouvée, dans l'ordre de [`CANDIDATES`], et le nom du fichier voisin qui la donne.
pub async fn find(client: &Client, url: &str, names: &[&str]) -> Option<(Checksum, String)> {
    let url = Url::parse(url).ok()?;
    let own = filename::name_from_url(&url)?;
    let mut all: Vec<String> = vec![own.clone()];
    all.extend(
        names
            .iter()
            .filter(|n| !n.is_empty() && **n != own)
            .map(|n| (*n).to_owned()),
    );
    let mut set = JoinSet::new();
    for (rank, (algo, kind)) in CANDIDATES.into_iter().enumerate() {
        let Some((at, origin)) = neighbor(&url, &own, kind) else {
            continue;
        };
        let (client, all) = (client.clone(), all.clone());
        set.spawn(async move {
            let body = client.fetch_small(&at, MAX_LEN).await.ok().flatten()?;
            let text = String::from_utf8_lossy(&body);
            let names: Vec<&str> = all.iter().map(String::as_str).collect();
            let alone = matches!(kind, Kind::Companion(_));
            find_in(&text, algo, &names, alone).map(|c| (rank, c, origin))
        });
    }
    let mut best: Option<(usize, Checksum, String)> = None;
    let gather = async {
        while let Some(done) = set.join_next().await {
            if let Ok(Some(found)) = done
                && best.as_ref().is_none_or(|b| found.0 < b.0)
            {
                best = Some(found);
            }
        }
    };
    // Délai dépassé : ce qui est arrivé compte ; les requêtes restantes s'abandonnent.
    let _ = tokio::time::timeout(TIMEOUT, gather).await;
    best.map(|(_, checksum, origin)| (checksum, origin))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const B: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    fn find256(text: &str, alone: bool) -> Option<String> {
        find_in(text, Algo::Sha256, &["debian-12.iso"], alone).map(|c| c.value().to_owned())
    }

    #[test]
    fn a_list_gives_the_line_of_the_file_and_only_it() {
        let list =
            format!("{B}  debian-12-edu.iso\n{A}  debian-12.iso\n{B} *autre.iso\n# commentaire\n");
        assert_eq!(find256(&list, false).as_deref(), Some(A));
        // Mode binaire, `./`, fins de ligne Windows, majuscules.
        let upper = A.to_ascii_uppercase();
        assert_eq!(
            find256(&format!("{upper} *./debian-12.iso\r\n"), false).as_deref(),
            Some(A)
        );
        // Format BSD.
        assert_eq!(
            find256(&format!("SHA256 (debian-12.iso) = {A}\n"), false).as_deref(),
            Some(A)
        );
        // Un nom voisin ne compte pas : ni préfixe, ni sous-dossier, ni autre algorithme.
        assert_eq!(
            find256(&format!("{A}  debian-12.iso.torrent\n"), false),
            None
        );
        assert_eq!(find256(&format!("{A}  amd64/debian-12.iso\n"), false), None);
        assert_eq!(
            find256(&format!("SHA1 (debian-12.iso) = {A}\n"), false),
            None
        );
    }

    #[test]
    fn a_bare_value_counts_only_in_a_companion_file() {
        assert_eq!(find256(&format!("{A}\n"), true).as_deref(), Some(A));
        assert_eq!(find256(&format!("{A}\n"), false), None);
        // Le compagnon d'un autre fichier ne vaut pas pour celui-ci.
        assert_eq!(find256(&format!("{A}  autre.iso\n"), true), None);
    }

    #[test]
    fn bad_or_conflicting_values_give_nothing() {
        // Longueur d'une autre empreinte (SHA-512 dans un `.sha256`), caractères non hexa.
        assert_eq!(find256(&format!("{A}{A}  debian-12.iso\n"), false), None);
        assert_eq!(find256("zz  debian-12.iso\n", false), None);
        // Ligne GNU échappée : ignorée.
        assert_eq!(find256(&format!("\\{A}  debian-12.iso\n"), false), None);
        // Deux empreintes différentes pour le même fichier : aucune.
        let both = format!("{A}  debian-12.iso\n{B}  ./debian-12.iso\n");
        assert_eq!(find256(&both, false), None);
        // La même deux fois : elle.
        let twice = format!("{A}  debian-12.iso\n{A} *debian-12.iso\n");
        assert_eq!(find256(&twice, false).as_deref(), Some(A));
    }

    #[test]
    fn neighbors_sit_next_to_the_file_without_its_query() {
        let url = Url::parse("https://exemple.org/pub/iso/debian-12.iso?jeton=1#x").unwrap();
        let (at, origin) = neighbor(&url, "debian-12.iso", Kind::Companion("sha256")).unwrap();
        assert_eq!(
            at.as_str(),
            "https://exemple.org/pub/iso/debian-12.iso.sha256"
        );
        assert_eq!(origin, "debian-12.iso.sha256");
        let (at, origin) = neighbor(&url, "debian-12.iso", Kind::List("SHA256SUMS")).unwrap();
        assert_eq!(at.as_str(), "https://exemple.org/pub/iso/SHA256SUMS");
        assert_eq!(origin, "SHA256SUMS");
        let dir = Url::parse("https://exemple.org/pub/").unwrap();
        assert!(neighbor(&dir, "pub", Kind::List("SHA256SUMS")).is_none());
    }
}
