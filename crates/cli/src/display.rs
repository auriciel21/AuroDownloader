//! Affichage : tailles, débits, temps restant, ligne d'avancement.

use std::io::{IsTerminal, Write};
use std::time::Instant;

/// Taille lisible, en unités binaires.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["o", "Kio", "Mio", "Gio", "Tio"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} o")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Limite de débit donnée en ligne de commande : `Some(octets par seconde)`, ou `None`
/// pour « aucune limite ».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit(pub Option<u64>);

/// Lit une empreinte (`--checksum`, `auro checksum`) : « sha256:… », « sha1:… »,
/// « md5:… », ou la valeur seule, dont la longueur donne l'algorithme.
pub fn parse_checksum(text: &str) -> Result<auro_engine::io::checksum::Checksum, String> {
    auro_engine::io::checksum::Checksum::from_text(text).map_err(|e| e.to_string())
}

/// Lit un en-tête « Nom: valeur » (`auro add -H`) ; le moteur vérifie le reste.
pub fn parse_header(text: &str) -> Result<(String, String), String> {
    let (name, value) = text
        .split_once(':')
        .ok_or_else(|| "« Nom: valeur » attendu".to_owned())?;
    let name = name.trim();
    if name.is_empty() {
        return Err("nom d'en-tête vide".into());
    }
    Ok((name.to_owned(), value.trim().to_owned()))
}

/// Lit un débit par seconde : « 800000 », « 500K », « 1.5M », « 2G » (unités binaires :
/// Kio, Mio, Gio), ou « none » pour aucune limite.
pub fn parse_limit(text: &str) -> Result<Limit, String> {
    let text = text.trim();
    if matches!(text.to_lowercase().as_str(), "none" | "aucune") {
        return Ok(Limit(None));
    }
    let (number, factor) = match text.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() => {
            let factor = match c.to_ascii_uppercase() {
                'K' => 1024.0,
                'M' => 1024.0 * 1024.0,
                'G' => 1024.0 * 1024.0 * 1024.0,
                _ => return Err(format!("unité inconnue : {c} (K, M ou G)")),
            };
            (&text[..i], factor)
        }
        _ => (text, 1.0),
    };
    let value: f64 = number
        .replace(',', ".")
        .parse()
        .map_err(|_| format!("débit illisible : {text} (exemples : 500K, 2M, none)"))?;
    let bps = (value * factor).round();
    if !bps.is_finite() || bps < 1.0 || bps > u64::MAX as f64 {
        return Err(format!("débit hors limites : {text}"));
    }
    Ok(Limit(Some(bps as u64)))
}

/// Limite de débit lisible.
pub fn limit(bps: Option<u64>) -> String {
    match bps {
        Some(bps) => format!("{}/s", size(bps)),
        None => "aucune".into(),
    }
}

/// Durée lisible : « 42 s », « 3 min 05 s », « 2 h 07 min ».
pub fn duration(seconds: u64) -> String {
    match seconds {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min {:02} s", s / 60, s % 60),
        s => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
    }
}

/// Avancement d'une tâche, tel qu'affiché.
pub struct Line {
    pub done: u64,
    pub total: Option<u64>,
    pub bps: u64,
    pub connections: u64,
    pub eta: Option<u64>,
    /// Torrent : `connections` compte des pairs.
    pub peers: bool,
    /// Torrent : débit d'envoi aux autres pairs.
    pub upload_bps: Option<u64>,
    /// Copie d'un site (SPEC §7.18 ter) : ses éléments, à la place des connexions.
    pub site: Option<SiteLine>,
}

/// Éléments d'une copie de site, tels que `task.progress` les donne.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SiteLine {
    pub pages: u64,
    pub resources: u64,
    pub files: u64,
    pub failed: u64,
    /// Liens laissés en ligne, pas des pages web : pas des échecs.
    pub online: u64,
    pub queued: u64,
}

impl SiteLine {
    /// Ceux de `site` d'un avancement ou d'une tâche, s'il y en a.
    pub fn of(value: &serde_json::Value) -> Option<SiteLine> {
        let n = |key: &str| value[key].as_u64().unwrap_or(0);
        value.is_object().then(|| SiteLine {
            pages: n("pages"),
            resources: n("resources"),
            files: n("files"),
            failed: n("failed"),
            online: n("online"),
            queued: n("queued"),
        })
    }

    /// « 12 page(s), 40 ressource(s), 2 fichier(s), 3 en échec, 1 laissé(s) en ligne ».
    pub fn render(&self) -> String {
        let mut text = format!(
            "{} page(s), {} ressource(s), {} fichier(s)",
            self.pages, self.resources, self.files
        );
        if self.failed > 0 {
            text.push_str(&format!(", {} en échec", self.failed));
        }
        if self.online > 0 {
            text.push_str(&format!(", {} laissé(s) en ligne", self.online));
        }
        text
    }
}

impl Line {
    pub fn render(&self) -> String {
        // Copie d'un site : volume inconnu jusqu'à la fin, éléments plutôt que connexions.
        if let Some(site) = &self.site {
            let queued = match site.queued {
                0 => String::new(),
                n => format!(", {n} en attente"),
            };
            return format!(
                "{}{queued}  {}  {}/s",
                site.render(),
                size(self.done),
                size(self.bps)
            );
        }
        let done = match self.total {
            Some(total) if total > 0 => format!(
                "{:5.1} %  {} / {}",
                self.done as f64 * 100.0 / total as f64,
                size(self.done),
                size(total)
            ),
            _ => size(self.done),
        };
        let eta = self
            .eta
            .map(|s| format!("  reste {}", duration(s)))
            .unwrap_or_default();
        let who = if self.peers {
            "pair(s)"
        } else {
            "connexion(s)"
        };
        let upload = self
            .upload_bps
            .map(|bps| format!("  envoi {}/s", size(bps)))
            .unwrap_or_default();
        format!(
            "{done}  {}/s  {} {who}{upload}{eta}",
            size(self.bps),
            self.connections
        )
    }
}

/// Ligne d'avancement réécrite sur place (sur un terminal), ou une ligne par mise à jour.
pub struct Status {
    terminal: bool,
    shown: usize,
}

impl Status {
    pub fn new() -> Self {
        Status {
            terminal: std::io::stderr().is_terminal(),
            shown: 0,
        }
    }

    /// Remplace le bloc affiché par `lines`.
    pub fn show(&mut self, lines: &[String]) {
        let mut err = std::io::stderr().lock();
        if self.terminal {
            // Remonte au début du bloc précédent et l'efface ligne par ligne.
            if self.shown > 0 {
                let _ = write!(err, "\x1b[{}A", self.shown);
            }
            for line in lines {
                let _ = writeln!(err, "\x1b[2K{line}");
            }
            for _ in lines.len()..self.shown {
                let _ = writeln!(err, "\x1b[2K");
            }
            self.shown = lines.len().max(self.shown);
        } else {
            for line in lines {
                let _ = writeln!(err, "{line}");
            }
        }
        let _ = err.flush();
    }

    /// Le bloc reste à l'écran ; ce qui suit s'écrit dessous.
    pub fn finish(&mut self) {
        self.shown = 0;
    }
}

/// Débit lissé, pour le mode direct, où le moteur ne le calcule pas.
pub struct Speed {
    last: Option<(Instant, u64)>,
    bps: f64,
}

impl Speed {
    pub fn new() -> Self {
        Speed {
            last: None,
            bps: 0.0,
        }
    }

    pub fn update(&mut self, received: u64) -> u64 {
        let now = Instant::now();
        if let Some((at, bytes)) = self.last {
            let elapsed = now.duration_since(at).as_secs_f64();
            if elapsed > 0.0 {
                let instant = received.saturating_sub(bytes) as f64 / elapsed;
                self.bps = if self.bps == 0.0 {
                    instant
                } else {
                    0.7 * self.bps + 0.3 * instant
                };
            }
        }
        self.last = Some((now, received));
        self.bps as u64
    }
}

/// Une ligne lue sur l'entrée standard, sans son retour à la ligne (mot de passe d'un
/// script, `--password-stdin`).
pub fn read_line_stdin() -> anyhow::Result<String> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

/// Champ CSV (RFC 4180) : entre guillemets s'il contient une virgule, un guillemet ou un
/// saut de ligne, les guillemets doublés.
pub fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_fields_are_quoted_only_when_needed() {
        assert_eq!(csv_field("debian.iso"), "debian.iso");
        assert_eq!(csv_field(""), "");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("dit \"oui\""), "\"dit \"\"oui\"\"\"");
        assert_eq!(csv_field("deux\nlignes"), "\"deux\nlignes\"");
    }

    #[test]
    fn limits_are_read_in_binary_units() {
        assert_eq!(parse_limit("800000"), Ok(Limit(Some(800_000))));
        assert_eq!(parse_limit("500K"), Ok(Limit(Some(512_000))));
        assert_eq!(parse_limit("1.5m"), Ok(Limit(Some(1_572_864))));
        assert_eq!(parse_limit("1,5M"), Ok(Limit(Some(1_572_864))));
        assert_eq!(parse_limit("2G"), Ok(Limit(Some(2 << 30))));
        assert_eq!(parse_limit("none"), Ok(Limit(None)));
        assert_eq!(parse_limit("Aucune"), Ok(Limit(None)));
        for bad in ["0", "0.1", "-5M", "vite", "5T", "", "M"] {
            assert!(parse_limit(bad).is_err(), "{bad}");
        }
        assert_eq!(limit(Some(2 * 1024 * 1024)), "2.0 Mio/s");
        assert_eq!(limit(None), "aucune");
    }

    #[test]
    fn headers_are_split_at_the_first_colon() {
        let pair = |n: &str, v: &str| Ok((n.to_owned(), v.to_owned()));
        assert_eq!(parse_header("Cookie: a=1; b=2"), pair("Cookie", "a=1; b=2"));
        assert_eq!(
            parse_header("Referer:https://exemple.org/p"),
            pair("Referer", "https://exemple.org/p")
        );
        assert_eq!(parse_header("X-Vide:"), pair("X-Vide", ""));
        for bad in ["Cookie", ": valeur", ""] {
            assert!(parse_header(bad).is_err(), "{bad}");
        }
    }
}
