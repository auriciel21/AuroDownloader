//! QR code de l'accès à distance (SPEC §7.19, étape 3d) : l'adresse de la page et le jeton
//! dans l'ancre, montrés à la création d'un appareil. Seule la matrice sort d'ici ; la page
//! la dessine elle-même (pas de balisage fabriqué ici).

use qrcode::{Color, EcLevel, QrCode};
use serde::Serialize;

/// Taille maximale du texte : une adresse et un jeton tiennent largement dedans.
const MAX_TEXT: usize = 512;

/// Matrice d'un QR code : `width` modules de côté, ligne par ligne, `1` pour un module
/// foncé.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Qr {
    pub width: usize,
    pub modules: String,
}

/// Le QR code d'un texte, en correction moyenne (lisible sur un écran un peu reflétant).
pub fn encode(text: &str) -> Result<Qr, String> {
    if text.is_empty() || text.len() > MAX_TEXT {
        return Err(format!("QR code : texte de 1 à {MAX_TEXT} octets"));
    }
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M)
        .map_err(|e| format!("QR code : {e}"))?;
    let modules = code
        .to_colors()
        .iter()
        .map(|c| if *c == Color::Dark { '1' } else { '0' })
        .collect();
    Ok(Qr {
        width: code.width(),
        modules,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_with_its_token_gives_a_square_matrix() {
        let text = "https://[fd00:1234:5678::20]:7380/#token=auro_\
                    abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ";
        let qr = encode(text).unwrap();
        assert_eq!(qr.modules.len(), qr.width * qr.width);
        assert!(qr.width >= 21 && qr.width % 4 == 1, "{}", qr.width);
        assert!(qr.modules.bytes().all(|b| b == b'0' || b == b'1'));
        // Les trois motifs de position : le coin en haut à gauche est foncé.
        assert_eq!(&qr.modules[..7], "1111111");
    }

    #[test]
    fn empty_or_huge_texts_are_refused() {
        assert!(encode("").is_err());
        assert!(encode(&"a".repeat(MAX_TEXT + 1)).is_err());
    }
}
