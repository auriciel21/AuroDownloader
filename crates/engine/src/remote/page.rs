//! Page de l'interface distante (SPEC §7.19, étape 3c) : les fichiers de `ui/src`, intégrés
//! au binaire à la compilation ; la même page que celle du bureau, avec l'hôte WebSocket de
//! `web.js` à la place de celui de Tauri.
//!
//! Servie sans authentification (elle ne contient aucune donnée) ; ce qu'elle montre passe
//! ensuite par le WebSocket, avec la session du navigateur.

/// Un fichier de la page.
#[derive(Debug)]
pub struct Asset {
    pub path: &'static str,
    pub content_type: &'static str,
    pub body: &'static [u8],
}

macro_rules! asset {
    ($name:literal, $type:literal) => {
        Asset {
            path: concat!("/", $name),
            content_type: $type,
            body: include_bytes!(concat!("../../../../ui/src/", $name)),
        }
    };
}

/// Les fichiers servis ; rien d'autre.
pub const ASSETS: &[Asset] = &[
    asset!("index.html", "text/html; charset=utf-8"),
    asset!("style.css", "text/css; charset=utf-8"),
    asset!("main.js", "text/javascript; charset=utf-8"),
    asset!("model.js", "text/javascript; charset=utf-8"),
    asset!("i18n.js", "text/javascript; charset=utf-8"),
    asset!("host.js", "text/javascript; charset=utf-8"),
    asset!("web.js", "text/javascript; charset=utf-8"),
    // Guide de l'utilisateur (étape 9d), lu par la boîte « Guide » ; fait par
    // scripts/guide.mjs, sans script.
    asset!("guide/fr.html", "text/html; charset=utf-8"),
    asset!("guide/en.html", "text/html; charset=utf-8"),
];

/// Le fichier d'un chemin (`/` : la page).
pub fn find(path: &str) -> Option<&'static Asset> {
    let path = if path == "/" { "/index.html" } else { path };
    ASSETS.iter().find(|a| a.path == path)
}

/// Politique de sécurité de la page (SPEC §7.19) : ses propres scripts et styles seulement,
/// aucun script ni style en ligne, connexions vers ce serveur seulement (`host` : la
/// valeur de `Host`, déjà vérifiée), aucun cadre.
pub fn csp(host: &str, tls: bool) -> String {
    let ws = if tls { "wss" } else { "ws" };
    format!(
        "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
         connect-src 'self' {ws}://{host}; base-uri 'none'; form-action 'none'; \
         frame-ancestors 'none'"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_page_files_are_found() {
        assert_eq!(find("/").unwrap().path, "/index.html");
        assert!(find("/main.js").is_some() && find("/web.js").is_some());
        assert!(find("/guide/fr.html").is_some() && find("/guide/en.html").is_some());
        assert!(find("/guide/").is_none() && find("/guide/../index.html").is_none());
        for missing in [
            "/../Cargo.toml",
            "/index.htm",
            "",
            "/ui/src/main.js",
            "/MAIN.JS",
        ] {
            assert!(find(missing).is_none(), "{missing}");
        }
        // La page charge ses modules par des chemins relatifs, tous servis.
        let html = std::str::from_utf8(find("/").unwrap().body).unwrap();
        assert!(html.contains("src=\"main.js\"") && html.contains("href=\"style.css\""));
        for asset in ASSETS {
            assert!(!asset.body.is_empty(), "{}", asset.path);
        }
    }
}
