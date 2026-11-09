//! Vérification d'une signature OpenPGP détachée (RFC 9580), réduite à ce que publie yt-dlp
//! (SPEC §7.17) : clé principale RSA en version 4, signature binaire (type `0x00`) en
//! version 4, SHA-256 ou SHA-512. Tout le reste est refusé plutôt que deviné.
//!
//! La vérification RSA elle-même est celle d'aws-lc (PKCS #1 v1.5) ; ce module ne fait que
//! lire les paquets et reconstituer le message signé.

use aws_lc_rs::signature::{
    RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_2048_8192_SHA512, RsaPublicKeyComponents,
};
use base64::Engine as _;
use sha1::{Digest as _, Sha1};
use sha2::{Sha256, Sha512};

/// Erreurs de lecture ou de vérification.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PgpError {
    #[error("format OpenPGP non reconnu : {0}")]
    Format(&'static str),
    #[error("non pris en charge : {0}")]
    Unsupported(&'static str),
    #[error("signature faite par une autre clé")]
    WrongKey,
    #[error("signature fausse")]
    BadSignature,
}

/// Clé publique RSA, avec son empreinte v4 (SHA-1 du paquet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey {
    pub n: Vec<u8>,
    pub e: Vec<u8>,
    pub fingerprint: [u8; 20],
}

impl PublicKey {
    /// Une clé donnée par ses composantes (tests : clé de test). L'empreinte est celle du
    /// paquet v4 qu'elle formerait, créé à `created` (secondes Unix).
    pub fn from_components(n: &[u8], e: &[u8], created: u32) -> PublicKey {
        let body = key_packet_body(n, e, created);
        PublicKey {
            n: strip(n).to_vec(),
            e: strip(e).to_vec(),
            fingerprint: fingerprint(&body),
        }
    }

    /// L'identifiant de la clé : les 8 derniers octets de l'empreinte.
    pub fn key_id(&self) -> [u8; 8] {
        self.fingerprint[12..].try_into().expect("8 octets")
    }
}

/// Lit une clé publique en armure ASCII : son premier paquet de clé principale.
pub fn parse_armored_key(text: &str) -> Result<PublicKey, PgpError> {
    let bytes = dearmor(text, "PGP PUBLIC KEY BLOCK")?;
    let (tag, body) = packets(&bytes)
        .next()
        .ok_or(PgpError::Format("aucun paquet"))??;
    if tag != 6 {
        return Err(PgpError::Format("pas une clé publique"));
    }
    parse_key_packet(body)
}

/// Vérifie que `signature` (paquet binaire, fichier `.sig`) signe `data` avec `key`.
pub fn verify_detached(key: &PublicKey, data: &[u8], signature: &[u8]) -> Result<(), PgpError> {
    let (tag, body) = packets(signature)
        .next()
        .ok_or(PgpError::Format("aucun paquet"))??;
    if tag != 2 {
        return Err(PgpError::Format("pas une signature"));
    }
    let sig = parse_signature(body)?;
    // L'émetteur doit être la clé attendue : empreinte (sous-paquet 33) ou identifiant
    // (sous-paquet 16), au moins l'un des deux.
    let mut named = false;
    for (kind, value) in sig.subpackets() {
        match kind {
            33 if value.len() == 21 && value[0] == 4 => {
                if value[1..] != key.fingerprint {
                    return Err(PgpError::WrongKey);
                }
                named = true;
            }
            16 if value.len() == 8 => {
                if value != key.key_id() {
                    return Err(PgpError::WrongKey);
                }
                named = true;
            }
            _ => {}
        }
    }
    if !named {
        return Err(PgpError::Format("signature sans émetteur"));
    }
    // Message signé : les données, la partie hachée de la signature, puis la fin v4.
    let mut message = Vec::with_capacity(data.len() + sig.hashed.len() + 6);
    message.extend_from_slice(data);
    message.extend_from_slice(sig.hashed);
    message.extend_from_slice(&[0x04, 0xff]);
    message.extend_from_slice(&(sig.hashed.len() as u32).to_be_bytes());
    let digest_start: [u8; 2] = match sig.hash {
        Hash::Sha256 => Sha256::digest(&message)[..2].try_into().expect("2 octets"),
        Hash::Sha512 => Sha512::digest(&message)[..2].try_into().expect("2 octets"),
    };
    if digest_start != sig.left16 {
        return Err(PgpError::BadSignature);
    }
    // La valeur RSA, complétée à la taille du module (l'entier OpenPGP perd ses zéros de
    // tête).
    if sig.value.len() > key.n.len() {
        return Err(PgpError::BadSignature);
    }
    let mut value = vec![0u8; key.n.len() - sig.value.len()];
    value.extend_from_slice(sig.value);
    let params = match sig.hash {
        Hash::Sha256 => &RSA_PKCS1_2048_8192_SHA256,
        Hash::Sha512 => &RSA_PKCS1_2048_8192_SHA512,
    };
    RsaPublicKeyComponents {
        n: &key.n,
        e: &key.e,
    }
    .verify(params, &message, &value)
    .map_err(|_| PgpError::BadSignature)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hash {
    Sha256,
    Sha512,
}

/// Signature v4 lue.
struct Signature<'a> {
    hash: Hash,
    /// De la version jusqu'à la fin des sous-paquets hachés : la partie signée.
    hashed: &'a [u8],
    /// Sous-paquets hachés puis non hachés.
    subpackets: [&'a [u8]; 2],
    left16: [u8; 2],
    value: &'a [u8],
}

impl<'a> Signature<'a> {
    /// Les sous-paquets (type sans le bit critique, valeur) ; un sous-paquet mal formé
    /// arrête la lecture.
    fn subpackets(&self) -> Vec<(u8, &'a [u8])> {
        let mut out = Vec::new();
        for area in self.subpackets {
            let mut rest = area;
            while !rest.is_empty() {
                let Some((len, used)) = subpacket_length(rest) else {
                    break;
                };
                let Some(packet) = rest.get(used..used + len) else {
                    break;
                };
                if let Some((&kind, value)) = packet.split_first() {
                    out.push((kind & 0x7f, value));
                }
                rest = &rest[used + len..];
            }
        }
        out
    }
}

fn parse_signature(body: &[u8]) -> Result<Signature<'_>, PgpError> {
    let short = PgpError::Format("signature tronquée");
    if body.first() != Some(&4) {
        return Err(PgpError::Unsupported("signature d'une autre version que 4"));
    }
    let [_, kind, algo, hash, a, b, ..] = *body else {
        return Err(short);
    };
    if kind != 0x00 {
        return Err(PgpError::Unsupported(
            "signature d'un autre type que binaire",
        ));
    }
    if algo != 1 {
        return Err(PgpError::Unsupported(
            "signature d'un autre algorithme que RSA",
        ));
    }
    let hash = match hash {
        8 => Hash::Sha256,
        10 => Hash::Sha512,
        _ => {
            return Err(PgpError::Unsupported(
                "empreinte autre que SHA-256 ou SHA-512",
            ));
        }
    };
    let hashed_len = usize::from(u16::from_be_bytes([a, b]));
    let hashed_end = 6 + hashed_len;
    let hashed = body
        .get(..hashed_end)
        .ok_or(PgpError::Format("signature tronquée"))?;
    let unhashed_len = body
        .get(hashed_end..hashed_end + 2)
        .map(|l| usize::from(u16::from_be_bytes([l[0], l[1]])))
        .ok_or(PgpError::Format("signature tronquée"))?;
    let unhashed_end = hashed_end + 2 + unhashed_len;
    let unhashed = body
        .get(hashed_end + 2..unhashed_end)
        .ok_or(PgpError::Format("signature tronquée"))?;
    let left16: [u8; 2] = body
        .get(unhashed_end..unhashed_end + 2)
        .and_then(|l| l.try_into().ok())
        .ok_or(PgpError::Format("signature tronquée"))?;
    let (value, _) = mpi(&body[unhashed_end + 2..])?;
    Ok(Signature {
        hash,
        hashed,
        subpackets: [&hashed[6..], unhashed],
        left16,
        value,
    })
}

fn parse_key_packet(body: &[u8]) -> Result<PublicKey, PgpError> {
    if body.first() != Some(&4) {
        return Err(PgpError::Unsupported("clé d'une autre version que 4"));
    }
    if body.get(5) != Some(&1) {
        return Err(PgpError::Unsupported("clé d'un autre algorithme que RSA"));
    }
    let (n, rest) = mpi(body.get(6..).ok_or(PgpError::Format("clé tronquée"))?)?;
    let (e, _) = mpi(rest)?;
    Ok(PublicKey {
        n: n.to_vec(),
        e: e.to_vec(),
        fingerprint: fingerprint(body),
    })
}

/// Corps d'un paquet de clé publique RSA v4.
fn key_packet_body(n: &[u8], e: &[u8], created: u32) -> Vec<u8> {
    let mut body = vec![4];
    body.extend_from_slice(&created.to_be_bytes());
    body.push(1);
    body.extend(encode_mpi(n));
    body.extend(encode_mpi(e));
    body
}

/// Empreinte v4 : SHA-1 de `0x99`, de la longueur du paquet sur deux octets et du paquet.
fn fingerprint(body: &[u8]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update([0x99]);
    hasher.update((body.len() as u16).to_be_bytes());
    hasher.update(body);
    hasher.finalize().into()
}

/// Un entier OpenPGP (MPI) : nombre de bits sur deux octets, puis l'entier ; rend l'entier
/// et la suite.
fn mpi(bytes: &[u8]) -> Result<(&[u8], &[u8]), PgpError> {
    let [a, b, ..] = *bytes else {
        return Err(PgpError::Format("entier tronqué"));
    };
    let len = usize::from(u16::from_be_bytes([a, b])).div_ceil(8);
    let value = bytes
        .get(2..2 + len)
        .ok_or(PgpError::Format("entier tronqué"))?;
    Ok((value, &bytes[2 + len..]))
}

fn strip(bytes: &[u8]) -> &[u8] {
    let zeros = bytes.iter().take_while(|&&b| b == 0).count();
    &bytes[zeros..]
}

fn encode_mpi(bytes: &[u8]) -> Vec<u8> {
    let bytes = strip(bytes);
    let bits = match bytes.first() {
        None => 0,
        Some(&top) => (bytes.len() - 1) * 8 + (8 - top.leading_zeros() as usize),
    };
    let mut out = (bits as u16).to_be_bytes().to_vec();
    out.extend_from_slice(bytes);
    out
}

/// Longueur d'un sous-paquet de signature, et le nombre d'octets qui la codent.
fn subpacket_length(bytes: &[u8]) -> Option<(usize, usize)> {
    match *bytes {
        [a, ..] if a < 192 => Some((usize::from(a), 1)),
        [a, b, ..] if a < 255 => Some((((usize::from(a) - 192) << 8) + usize::from(b) + 192, 2)),
        [255, a, b, c, d, ..] => Some((u32::from_be_bytes([a, b, c, d]) as usize, 5)),
        _ => None,
    }
}

/// Les paquets d'un flux binaire : (étiquette, corps). Formats ancien et nouveau ; pas de
/// longueur partielle ni indéterminée.
fn packets(bytes: &[u8]) -> impl Iterator<Item = Result<(u8, &[u8]), PgpError>> {
    let mut rest = bytes;
    std::iter::from_fn(move || {
        let (&ctb, after) = rest.split_first()?;
        let parsed = (|| {
            if ctb & 0x80 == 0 {
                return Err(PgpError::Format("en-tête de paquet invalide"));
            }
            let (tag, len, used) = if ctb & 0x40 == 0 {
                let tag = (ctb >> 2) & 0x0f;
                let (len, used) = match (ctb & 3, after) {
                    (0, [a, ..]) => (usize::from(*a), 1),
                    (1, [a, b, ..]) => (usize::from(u16::from_be_bytes([*a, *b])), 2),
                    (2, [a, b, c, d, ..]) => (u32::from_be_bytes([*a, *b, *c, *d]) as usize, 4),
                    _ => return Err(PgpError::Unsupported("longueur de paquet indéterminée")),
                };
                (tag, len, used)
            } else {
                let tag = ctb & 0x3f;
                let (len, used) = match *after {
                    [a, ..] if a < 192 => (usize::from(a), 1),
                    [a, b, ..] if a < 224 => {
                        (((usize::from(a) - 192) << 8) + usize::from(b) + 192, 2)
                    }
                    [255, a, b, c, d, ..] => (u32::from_be_bytes([a, b, c, d]) as usize, 5),
                    _ => return Err(PgpError::Unsupported("longueur de paquet partielle")),
                };
                (tag, len, used)
            };
            let body = after
                .get(used..used + len)
                .ok_or(PgpError::Format("paquet tronqué"))?;
            Ok((tag, body, used + len))
        })();
        match parsed {
            Ok((tag, body, consumed)) => {
                rest = &after[consumed..];
                Some(Ok((tag, body)))
            }
            Err(e) => {
                rest = &[];
                Some(Err(e))
            }
        }
    })
}

/// Décode une armure ASCII de ce genre (`-----BEGIN <kind>-----`), sans en vérifier la
/// somme CRC-24, que la signature rend inutile.
fn dearmor(text: &str, kind: &str) -> Result<Vec<u8>, PgpError> {
    let begin = format!("-----BEGIN {kind}-----");
    let end = format!("-----END {kind}-----");
    let mut lines = text.lines().map(str::trim);
    lines
        .find(|l| *l == begin)
        .ok_or(PgpError::Format("début d'armure absent"))?;
    // En-têtes jusqu'à la première ligne vide.
    for line in lines.by_ref() {
        if line.is_empty() {
            break;
        }
    }
    let mut data = String::new();
    for line in lines {
        if line == end {
            return base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| PgpError::Format("armure illisible"));
        }
        if line.starts_with('=') {
            continue;
        }
        data.push_str(line);
    }
    Err(PgpError::Format("fin d'armure absente"))
}

/// Pour les tests : une signature v4 binaire de `data` par une clé RSA de test.
#[cfg(any(test, feature = "testing"))]
pub fn sign_for_tests(
    key_pair: &aws_lc_rs::signature::RsaKeyPair,
    public: &PublicKey,
    data: &[u8],
) -> Vec<u8> {
    let mut hashed = vec![4, 0x00, 1, 10];
    let mut subpackets = Vec::new();
    // Date de création (2) et empreinte de l'émetteur (33).
    subpackets.extend_from_slice(&[5, 2]);
    subpackets.extend_from_slice(&1_700_000_000u32.to_be_bytes());
    subpackets.extend_from_slice(&[22, 33, 4]);
    subpackets.extend_from_slice(&public.fingerprint);
    hashed.extend_from_slice(&(subpackets.len() as u16).to_be_bytes());
    hashed.extend_from_slice(&subpackets);
    let mut message = data.to_vec();
    message.extend_from_slice(&hashed);
    message.extend_from_slice(&[0x04, 0xff]);
    message.extend_from_slice(&(hashed.len() as u32).to_be_bytes());
    let digest = Sha512::digest(&message);
    let rng = aws_lc_rs::rand::SystemRandom::new();
    let mut value = vec![0u8; key_pair.public_modulus_len()];
    key_pair
        .sign(
            &aws_lc_rs::signature::RSA_PKCS1_SHA512,
            &rng,
            &message,
            &mut value,
        )
        .expect("signature de test");
    let mut body = hashed;
    body.extend_from_slice(&[0, 0]);
    body.extend_from_slice(&digest[..2]);
    body.extend(encode_mpi(&value));
    // Paquet au nouveau format, longueur sur deux octets.
    let len = body.len() - 192;
    let mut packet = vec![0xc2, (len >> 8) as u8 + 192, len as u8];
    packet.extend(body);
    packet
}

#[cfg(test)]
mod tests {
    use super::*;

    const YTDLP_KEY: &str = include_str!("yt-dlp-public.key");
    const SUMS: &[u8] = include_bytes!("testdata/SHA2-256SUMS");
    const SUMS_SIG: &[u8] = include_bytes!("testdata/SHA2-256SUMS.sig");
    /// Empreinte de la clé de signature de yt-dlp, relevée le 2026-11-09 par `gpg`.
    const YTDLP_FINGERPRINT: &str = "AC0CBBE6848D6A873464AF4E57CF65933B5A7581";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02X}")).collect()
    }

    #[test]
    fn the_ytdlp_key_is_read_with_its_fingerprint() {
        let key = parse_armored_key(YTDLP_KEY).unwrap();
        assert_eq!(hex(&key.fingerprint), YTDLP_FINGERPRINT);
        assert_eq!(key.n.len(), 512, "RSA 4096");
    }

    #[test]
    fn a_real_ytdlp_release_signature_is_accepted() {
        let key = parse_armored_key(YTDLP_KEY).unwrap();
        verify_detached(&key, SUMS, SUMS_SIG).unwrap();
    }

    #[test]
    fn a_changed_byte_is_refused() {
        let key = parse_armored_key(YTDLP_KEY).unwrap();
        let mut sums = SUMS.to_vec();
        sums[3] ^= 1;
        assert_eq!(
            verify_detached(&key, &sums, SUMS_SIG),
            Err(PgpError::BadSignature)
        );
        let mut sig = SUMS_SIG.to_vec();
        let last = sig.len() - 1;
        sig[last] ^= 1;
        assert_eq!(
            verify_detached(&key, SUMS, &sig),
            Err(PgpError::BadSignature)
        );
        // Une fin de ligne Windows suffit à refuser (fichier recopié par Git…).
        let crlf = String::from_utf8(SUMS.to_vec())
            .unwrap()
            .replace('\n', "\r\n");
        assert!(verify_detached(&key, crlf.as_bytes(), SUMS_SIG).is_err());
    }

    #[test]
    fn another_key_is_refused() {
        let pair =
            aws_lc_rs::signature::RsaKeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).unwrap();
        let components = aws_lc_rs::rsa::PublicKeyComponents::<Vec<u8>>::from(
            aws_lc_rs::signature::KeyPair::public_key(&pair),
        );
        let test_key = PublicKey::from_components(&components.n, &components.e, 1_700_000_000);
        assert_eq!(
            verify_detached(&test_key, SUMS, SUMS_SIG),
            Err(PgpError::WrongKey)
        );
        // La clé de test signe et se vérifie ; la clé de yt-dlp ne l'accepte pas.
        let sig = sign_for_tests(&pair, &test_key, b"contenu");
        verify_detached(&test_key, b"contenu", &sig).unwrap();
        assert_eq!(
            verify_detached(&test_key, b"autre", &sig),
            Err(PgpError::BadSignature)
        );
        let ytdlp = parse_armored_key(YTDLP_KEY).unwrap();
        assert_eq!(
            verify_detached(&ytdlp, b"contenu", &sig),
            Err(PgpError::WrongKey)
        );
    }

    #[test]
    fn garbage_is_refused_without_panic() {
        let key = parse_armored_key(YTDLP_KEY).unwrap();
        for bad in [
            &b""[..],
            b"\x00",
            b"\x89\x02",
            b"\xc2\xff\x00",
            &SUMS_SIG[..40],
        ] {
            assert!(verify_detached(&key, SUMS, bad).is_err());
        }
        assert!(parse_armored_key("rien").is_err());
    }
}
