//! Miroirs d'une tâche HTTP (SPEC §7.7, phase 6, étape 1) : validation d'un miroir face à
//! la source de référence, en-têtes envoyés à chaque source.
//!
//! Décision de l'utilisateur du 2026-11-10 : un miroir est accepté s'il accepte les plages,
//! annonce la même taille que la référence et rend les mêmes octets sur 3 morceaux de
//! 16 Kio tirés au hasard (tout le fichier s'il est plus petit). Les validateurs (`ETag`,
//! date) diffèrent d'un serveur à l'autre : ils ne servent qu'à la reprise sur chaque source.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

use crate::net::auth::Auth;
use crate::net::{Client, NetError, RangeRequest};

/// Taille d'un échantillon comparé.
pub const SAMPLE_BYTES: u64 = 16 * 1024;
/// Nombre d'échantillons comparés.
pub const SAMPLES: usize = 3;

/// Une source vue par la validation : son adresse, ses en-têtes, ses identifiants, et le
/// validateur `If-Range` à présenter (celui mémorisé pour la référence).
pub struct Side<'a> {
    pub url: &'a str,
    pub headers: &'a [(String, String)],
    pub auth: Option<&'a Auth>,
    pub if_range: Option<&'a str>,
}

/// Ce qu'un miroir accepté a montré : ses validateurs, pour la reprise sur lui.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_length: u64,
}

/// Issue de la validation d'un miroir.
#[derive(Debug)]
pub enum Verdict {
    Accepted(Facts),
    /// Pas le même fichier, ou pas de plages : jamais utilisé.
    Refused(String),
    /// Injoignable pour l'instant : à revalider plus tard.
    Unreachable(NetError),
    /// La référence elle-même ne répond pas comme prévu (fichier changé, injoignable) : le
    /// miroir n'est pas en cause.
    Reference(NetError),
}

/// Valide le miroir `mirror` face à `reference`, pour un fichier de `total` octets.
pub async fn validate(
    client: &Client,
    reference: &Side<'_>,
    mirror: &Side<'_>,
    total: u64,
) -> Verdict {
    let probe = match client
        .probe_as(mirror.url, mirror.headers, mirror.auth)
        .await
    {
        Ok(probe) => probe,
        Err(e) if e.retryable() => return Verdict::Unreachable(e),
        Err(e) => return Verdict::Refused(format!("le miroir répond : {e}")),
    };
    if probe.total_bytes != Some(total) {
        return Verdict::Refused(match probe.total_bytes {
            Some(found) => format!("taille différente : {found} octets au lieu de {total}"),
            None => "taille inconnue".to_owned(),
        });
    }
    if !probe.accepts_ranges {
        return Verdict::Refused("le miroir n'accepte pas les requêtes par plage".to_owned());
    }
    let mirror_if_range = crate::net::headers::if_range_validator(
        probe.etag.as_deref(),
        probe.last_modified.as_deref(),
    );
    for (first, last) in sample_ranges(total, random()) {
        let expected = match read_range(client, reference, first, last, total).await {
            Ok(bytes) => bytes,
            Err(e) => return Verdict::Reference(e),
        };
        let mirror_side = Side {
            if_range: mirror_if_range.as_deref(),
            ..*mirror
        };
        let found = match read_range(client, &mirror_side, first, last, total).await {
            Ok(bytes) => bytes,
            Err(e) if e.retryable() => return Verdict::Unreachable(e),
            Err(e) => return Verdict::Refused(format!("le miroir répond : {e}")),
        };
        if found != expected {
            return Verdict::Refused(format!(
                "contenu différent de la référence (octets {first} à {last})"
            ));
        }
    }
    Verdict::Accepted(Facts {
        etag: probe.etag,
        last_modified: probe.last_modified,
        content_length: total,
    })
}

fn random() -> u64 {
    RandomState::new().build_hasher().finish()
}

/// Morceaux comparés : tout le fichier s'il tient dans les échantillons, sinon
/// [`SAMPLES`] morceaux de [`SAMPLE_BYTES`] à des positions tirées de `seed`, le premier
/// au début du fichier (là où un autre fichier de même taille diffère presque toujours).
pub fn sample_ranges(total: u64, seed: u64) -> Vec<(u64, u64)> {
    if total == 0 {
        return Vec::new();
    }
    if total <= SAMPLE_BYTES * SAMPLES as u64 {
        return vec![(0, total - 1)];
    }
    let span = total - SAMPLE_BYTES + 1;
    let mut state = seed;
    let mut ranges = vec![(0, SAMPLE_BYTES - 1)];
    for _ in 1..SAMPLES {
        // xorshift : assez pour tirer des positions, pas pour de la cryptographie.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let first = state % span;
        ranges.push((first, first + SAMPLE_BYTES - 1));
    }
    ranges
}

/// Octets `first..=last` d'une source, en mémoire.
async fn read_range(
    client: &Client,
    side: &Side<'_>,
    first: u64,
    last: u64,
    total: u64,
) -> Result<Vec<u8>, NetError> {
    let request = RangeRequest {
        first,
        last,
        total,
        if_range: side.if_range,
    };
    let mut body = client
        .get_range_as(side.url, side.headers, &request, side.auth)
        .await?;
    let mut bytes = Vec::with_capacity((last - first + 1) as usize);
    while let Some(chunk) = body.chunk().await? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() as u64 > last - first + 1 {
            return Err(NetError::BadResponse("plage plus longue que demandée"));
        }
    }
    if bytes.len() as u64 != last - first + 1 {
        return Err(NetError::BadResponse("plage incomplète"));
    }
    Ok(bytes)
}

/// En-têtes de la tâche envoyés à une source (SPEC §7.7) : tous vers le même hôte que la
/// référence ; vers un autre hôte, le seul `User-Agent` (ni cookie, ni `Referer`, ni
/// `Authorization` ne partent ailleurs).
pub fn headers_for(
    task_headers: &[(String, String)],
    reference_url: &str,
    url: &str,
) -> Vec<(String, String)> {
    let same_host = crate::transfer::host_of(reference_url) == crate::transfer::host_of(url);
    task_headers
        .iter()
        .filter(|(name, _)| same_host || name.eq_ignore_ascii_case("user-agent"))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests;
