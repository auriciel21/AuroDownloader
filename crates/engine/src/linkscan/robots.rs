//! robots.txt (RFC 9309) d'un site exploré (F-42, SPEC §7.18 bis) : les pages qu'il demande
//! aux robots de ne pas lire.
//!
//! Groupe retenu : celui qui nomme `AuroDownloader` (sans casse), sinon `*` ; plusieurs
//! groupes du même nom s'additionnent. Règles `Allow` et `Disallow`, `*` et `$` ; la règle
//! la plus longue l'emporte, `Allow` à égalité. `Crawl-delay` (hors RFC) est lu aussi.

use std::time::Duration;

use reqwest::Url;

/// Nom de produit cherché dans les lignes `User-agent` (celui de `net::USER_AGENT`).
pub const PRODUCT: &str = "aurodownloader";
/// Règles retenues au plus ; au-delà, le fichier est lu jusque-là.
const MAX_RULES: usize = 10_000;
/// Motif le plus long retenu, en octets.
const MAX_PATTERN: usize = 2048;

/// Les règles qui s'appliquent à AuroDownloader.
#[derive(Debug, Clone, Default)]
pub struct Robots {
    rules: Vec<Rule>,
    /// `Crawl-delay` du groupe retenu.
    pub crawl_delay: Option<Duration>,
}

#[derive(Debug, Clone)]
struct Rule {
    allow: bool,
    /// Motif normalisé (voir [`normalize`]).
    pattern: String,
}

#[derive(Default)]
struct Group {
    agents: Vec<String>,
    rules: Vec<Rule>,
    delay: Option<Duration>,
}

impl Robots {
    /// Tout est permis (fichier absent, réponse `4xx`, ou case « Ignorer robots.txt »).
    pub fn allow_all() -> Robots {
        Robots::default()
    }

    /// Rien n'est permis (réponse `5xx`, site injoignable : RFC 9309, §2.3.1.4).
    pub fn deny_all() -> Robots {
        Robots {
            rules: vec![Rule {
                allow: false,
                pattern: "/".into(),
            }],
            crawl_delay: None,
        }
    }

    /// Lit un fichier robots.txt.
    pub fn parse(text: &str) -> Robots {
        let mut groups: Vec<Group> = Vec::new();
        // La ligne précédente était un `User-agent` : un autre s'ajoute au même groupe.
        let mut in_agents = false;
        let mut count = 0usize;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or_default();
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "user-agent" => {
                    if !in_agents {
                        groups.push(Group::default());
                    }
                    in_agents = true;
                    // Le nom de produit seul, sans sa version (`AuroDownloader/1.0`).
                    let agent = value
                        .split(|c: char| c == '/' || c.is_whitespace())
                        .next()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    if let Some(group) = groups.last_mut() {
                        group.agents.push(agent);
                    }
                }
                "allow" | "disallow" => {
                    in_agents = false;
                    // Une règle vide ne dit rien ; une règle hors de tout groupe est ignorée.
                    if value.is_empty() || value.len() > MAX_PATTERN || count >= MAX_RULES {
                        continue;
                    }
                    if let Some(group) = groups.last_mut() {
                        group.rules.push(Rule {
                            allow: key == "allow",
                            pattern: normalize(value),
                        });
                        count += 1;
                    }
                }
                "crawl-delay" => {
                    in_agents = false;
                    let delay = value
                        .parse::<f64>()
                        .ok()
                        .filter(|s| s.is_finite() && *s >= 0.0)
                        .map(|s| Duration::from_secs_f64(s.min(3600.0)));
                    if let (Some(group), Some(delay)) = (groups.last_mut(), delay) {
                        group.delay = Some(delay);
                    }
                }
                // `Sitemap` et les autres lignes ne changent rien aux groupes.
                _ => {}
            }
        }
        let named: Vec<&Group> = groups
            .iter()
            .filter(|g| g.agents.iter().any(|a| a == PRODUCT))
            .collect();
        let chosen = if named.is_empty() {
            groups
                .iter()
                .filter(|g| g.agents.iter().any(|a| a == "*"))
                .collect()
        } else {
            named
        };
        Robots {
            rules: chosen
                .iter()
                .flat_map(|g| g.rules.iter().cloned())
                .collect(),
            crawl_delay: chosen.iter().filter_map(|g| g.delay).max(),
        }
    }

    /// La page `url` peut-elle être lue ?
    pub fn allows(&self, url: &Url) -> bool {
        let mut target = url.path().to_owned();
        if target == "/robots.txt" {
            return true;
        }
        if let Some(query) = url.query() {
            target.push('?');
            target.push_str(query);
        }
        let target = normalize(&target);
        let mut best: Option<(usize, bool)> = None;
        for rule in &self.rules {
            if !matches(&rule.pattern, &target) {
                continue;
            }
            let len = rule.pattern.len();
            best = match best {
                Some((l, allow)) if l > len || (l == len && allow) => Some((l, allow)),
                _ => Some((len, rule.allow)),
            };
        }
        best.is_none_or(|(_, allow)| allow)
    }
}

/// Forme comparable d'un chemin ou d'un motif : octets hors ASCII et espaces encodés
/// (`%XX`), chiffres hexadécimaux des encodages en majuscules (RFC 9309, §2.2.2).
fn normalize(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            out.push('%');
            out.push(bytes[i + 1].to_ascii_uppercase() as char);
            out.push(bytes[i + 2].to_ascii_uppercase() as char);
            i += 3;
            continue;
        }
        if b.is_ascii() && !b.is_ascii_whitespace() && !b.is_ascii_control() {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
        i += 1;
    }
    out
}

/// Le motif couvre-t-il le début de `target` (tout `target` s'il finit par `$`) ? `*` vaut
/// toute suite d'octets.
fn matches(pattern: &str, target: &str) -> bool {
    let (pattern, anchored) = match pattern.strip_suffix('$') {
        Some(p) => (p.as_bytes(), true),
        None => (pattern.as_bytes(), false),
    };
    let target = target.as_bytes();
    // Glob classique, avec retour à la dernière étoile ; sans ancre, le motif finit par
    // une étoile implicite.
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    loop {
        if p == pattern.len() {
            if !anchored || t == target.len() {
                return true;
            }
        } else if pattern[p] == b'*' {
            star = Some((p, t));
            p += 1;
            continue;
        } else if t < target.len() && pattern[p] == target[t] {
            p += 1;
            t += 1;
            continue;
        }
        match star {
            Some((sp, st)) if st < target.len() => {
                star = Some((sp, st + 1));
                p = sp + 1;
                t = st + 1;
            }
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests;
