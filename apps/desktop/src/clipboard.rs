//! Liens copiés dans le presse-papiers (F-30, SPEC §7.18, étape 2c), désactivé par défaut.
//!
//! Décisions de l'utilisateur (2026-11-11) : seule une adresse copiée seule qui a l'air d'un
//! fichier (extension hors pages web) ou un lien magnet fait apparaître une notification du
//! système ; un clic sur elle ouvre la boîte « Ajouter » préremplie, rien n'est ajouté
//! d'office ; le réglage est une préférence de l'interface (Options, Général).
//!
//! Le presse-papiers est lu une fois par seconde tant que l'option est cochée. Son contenu
//! n'est ni gardé ni écrit nulle part : seule une empreinte du dernier texte vu reste en
//! mémoire, pour reconnaître une nouvelle copie (SPEC §8.2). Une copie faite dans
//! AuroDownloader (fenêtre au premier plan) ne déclenche rien.
//!
//! Notifications : `tauri-winrt-notification` sous Windows et `notify-rust` sous Linux,
//! directement, parce que le greffon de notification de Tauri ne transmet pas le clic sur
//! ordinateur.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use tauri::{AppHandle, Url};

use crate::opened::is_magnet;

/// Intervalle de lecture du presse-papiers.
const EVERY: Duration = Duration::from_secs(1);
/// Texte copié examiné au plus : au-delà, ce n'est pas une adresse seule.
const MAX_TEXT: usize = 8 * 1024;

/// Extensions des pages web, comme celles du moteur (`linkscan`, SPEC §7.18).
const PAGE_EXTENSIONS: [&str; 17] = [
    "htm", "html", "xhtml", "shtml", "php", "php3", "php5", "asp", "aspx", "jsp", "jspx", "cgi",
    "pl", "cfm", "do", "action", "py",
];

/// Le lien à proposer pour un texte copié : une adresse HTTP(S) seule dont le dernier
/// segment a une extension hors pages web, ou un lien magnet ; `None` pour tout le reste
/// (page web, texte, plusieurs adresses).
pub fn candidate(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_TEXT || text.contains(char::is_whitespace) {
        return None;
    }
    if is_magnet(text) {
        return text.contains("xt=").then(|| text.to_owned());
    }
    let url = Url::parse(text).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    let last = url.path_segments()?.rev().find(|s| !s.is_empty())?;
    let (stem, ext) = last.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    let file = !stem.is_empty()
        && (1..=10).contains(&ext.len())
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
        && !PAGE_EXTENSIONS.contains(&ext.as_str());
    file.then(|| text.to_owned())
}

/// Nom montré dans la notification : le fichier (dernier segment, décodé), ou le nom d'un
/// lien magnet (`dn`) ; raccourci à 80 caractères.
pub fn shown_name(link: &str) -> String {
    let name = Url::parse(link).ok().and_then(|url| {
        if is_magnet(link) {
            url.query_pairs()
                .find(|(k, _)| k == "dn")
                .map(|(_, v)| v.into_owned())
        } else {
            url.path_segments()?
                .rev()
                .find(|s| !s.is_empty())
                .map(percent_decode)
        }
    });
    let name = name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "magnet".to_owned());
    let mut short: String = name.chars().filter(|c| !c.is_control()).take(80).collect();
    if name.chars().count() > 80 {
        short.push('…');
    }
    short
}

/// `%C3%A9` → `é` ; une séquence invalide reste telle quelle.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (
                bytes.get(i + 1).copied().and_then(hex),
                bytes.get(i + 2).copied().and_then(hex),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Titre, texte et libellé du bouton de la notification, dans la langue de l'interface.
fn texts(lang: &str, name: &str) -> (String, String, &'static str) {
    if lang == "fr" {
        (
            "Lien copié".to_owned(),
            format!("{name} : cliquez pour l'ajouter à AuroDownloader"),
            "Ajouter",
        )
    } else {
        (
            "Link copied".to_owned(),
            format!("{name}: click to add it to AuroDownloader"),
            "Add",
        )
    }
}

fn fingerprint(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Lance la surveillance, dans son propre fil, pour toute la vie de l'application.
/// `settings` dit si l'option est cochée et donne la langue (`Some(lang)`), ou `None`
/// quand elle ne l'est pas ; `focused`, si la fenêtre est au premier plan ; `chosen`
/// reçoit le lien d'une notification cliquée.
pub fn spawn<S, F, C>(app: AppHandle, settings: S, focused: F, chosen: C)
where
    S: Fn(&AppHandle) -> Option<&'static str> + Send + 'static,
    F: Fn(&AppHandle) -> bool + Send + 'static,
    C: Fn(&AppHandle, String) + Send + Sync + Clone + 'static,
{
    let _ = std::thread::Builder::new()
        .name("presse-papiers".into())
        .spawn(move || {
            let mut board: Option<arboard::Clipboard> = None;
            // Empreinte du dernier texte vu ; `None` : option coupée, le prochain texte lu
            // n'est que noté (ce qui était déjà copié ne déclenche rien).
            let mut last: Option<u64> = None;
            loop {
                std::thread::sleep(EVERY);
                let Some(lang) = settings(&app) else {
                    // Option coupée : le presse-papiers est rendu au système.
                    board = None;
                    last = None;
                    continue;
                };
                if board.is_none() {
                    board = arboard::Clipboard::new().ok();
                }
                let Some(clip) = board.as_mut() else {
                    continue;
                };
                // Pas de texte (image, fichiers, presse-papiers vide) : une valeur à part.
                let text = clip.get_text().unwrap_or_default();
                let seen = fingerprint(&text);
                match last.replace(seen) {
                    // Première lecture depuis que l'option est cochée : notée seulement.
                    None => continue,
                    // Rien de nouveau.
                    Some(previous) if previous == seen => continue,
                    Some(_) => {}
                }
                if focused(&app) {
                    continue;
                }
                if let Some(link) = candidate(&text) {
                    notify(&app, link, lang, chosen.clone());
                }
            }
        });
}

#[cfg(windows)]
fn notify<C>(app: &AppHandle, link: String, lang: &str, chosen: C)
where
    C: Fn(&AppHandle, String) + Send + Sync + Clone + 'static,
{
    use tauri_winrt_notification::Toast;
    let (title, body, _) = texts(lang, &shown_name(&link));
    let app_id = windows_app_id(app);
    let app = app.clone();
    let _ = Toast::new(&app_id)
        .title(&title)
        .text1(&body)
        .on_activated(move |_| {
            chosen(&app, link.clone());
            Ok(())
        })
        .show();
}

/// Identifiant de l'application pour Windows : le sien une fois installée ; celui de
/// PowerShell pour un exécutable lancé depuis `target` (essais), seul à s'afficher sans
/// installation (même règle que le greffon de notification de Tauri).
#[cfg(windows)]
fn windows_app_id(app: &AppHandle) -> String {
    let from_build = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.to_path_buf()))
        .is_some_and(|dir| dir.ends_with("target\\debug") || dir.ends_with("target\\release"));
    if from_build {
        tauri_winrt_notification::Toast::POWERSHELL_APP_ID.to_owned()
    } else {
        app.config().identifier.clone()
    }
}

/// Vrai quand Windows n'affichera pas nos notifications : interrupteur général coupé
/// (`PushNotifications\ToastEnabled`), ou celles de l'application coupées
/// (`Notifications\Settings\<identifiant>\Enabled`). Lecture seule ; une valeur absente
/// vaut « activées », comme pour Windows. Les Options en avertissent (l'option ne ferait
/// rien sans le dire).
#[cfg(windows)]
pub fn notifications_blocked(app: &AppHandle) -> bool {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let off = |path: &str, name: &str| {
        hkcu.open_subkey_with_flags(path, KEY_READ)
            .and_then(|key| key.get_value::<u32, _>(name))
            .is_ok_and(|value| value == 0)
    };
    let base = r"Software\Microsoft\Windows\CurrentVersion";
    off(&format!(r"{base}\PushNotifications"), "ToastEnabled")
        || off(
            &format!(r"{base}\Notifications\Settings\{}", windows_app_id(app)),
            "Enabled",
        )
}

/// Ailleurs, rien de simple à lire : jamais d'avertissement.
#[cfg(not(windows))]
pub fn notifications_blocked(_app: &AppHandle) -> bool {
    false
}

/// Notifications attendues à la fois (Linux) : chacune tient un fil jusqu'à sa fermeture.
#[cfg(any(test, target_os = "linux"))]
const MAX_WAITING: usize = 4;

/// Places des notifications attendues : un serveur qui ne les ferme jamais ne fait pas
/// naître un fil par lien copié (audit C19).
#[cfg(any(test, target_os = "linux"))]
struct Waiting(std::sync::atomic::AtomicUsize);

/// Une place prise, rendue quand le fil finit.
#[cfg(any(test, target_os = "linux"))]
struct Slot<'a>(&'a Waiting);

#[cfg(any(test, target_os = "linux"))]
impl Waiting {
    const fn new() -> Self {
        Waiting(std::sync::atomic::AtomicUsize::new(0))
    }

    fn take(&self) -> Option<Slot<'_>> {
        use std::sync::atomic::Ordering;
        let mut n = self.0.load(Ordering::SeqCst);
        while n < MAX_WAITING {
            match self
                .0
                .compare_exchange(n, n + 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Some(Slot(self)),
                Err(now) => n = now,
            }
        }
        None
    }
}

#[cfg(any(test, target_os = "linux"))]
impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(target_os = "linux")]
fn notify<C>(app: &AppHandle, link: String, lang: &str, chosen: C)
where
    C: Fn(&AppHandle, String) + Send + Sync + Clone + 'static,
{
    static WAITING: Waiting = Waiting::new();
    // Toutes les places prises : pas de notification, le lien reste dans le presse-papiers.
    let Some(slot) = WAITING.take() else {
        return;
    };
    let (title, body, button) = texts(lang, &shown_name(&link));
    let app = app.clone();
    // `wait_for_action` attend la fermeture de la notification : un fil à part.
    let _ = std::thread::Builder::new()
        .name("notification".into())
        .spawn(move || {
            let _slot = slot;
            let shown = notify_rust::Notification::new()
                .appname("AuroDownloader")
                .summary(&title)
                .body(&body)
                .action("default", button)
                .timeout(notify_rust::Timeout::Milliseconds(10_000))
                .show();
            if let Ok(handle) = shown {
                handle.wait_for_action(|action| {
                    if action == "default" {
                        chosen(&app, link.clone());
                    }
                });
            }
        });
}

#[cfg(not(any(windows, target_os = "linux")))]
fn notify<C>(_app: &AppHandle, _link: String, _lang: &str, _chosen: C)
where
    C: Fn(&AppHandle, String) + Send + Sync + Clone + 'static,
{
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audit C19 : au plus quatre notifications attendues à la fois ; une place revient
    /// quand son fil finit.
    #[test]
    fn waiting_notifications_are_bounded() {
        let waiting = Waiting::new();
        let mut slots: Vec<_> = (0..MAX_WAITING).map(|_| waiting.take().unwrap()).collect();
        assert!(waiting.take().is_none());
        slots.pop();
        let again = waiting.take();
        assert!(again.is_some());
        assert!(waiting.take().is_none());
    }

    #[test]
    fn only_a_lone_file_address_or_a_magnet_link_is_proposed() {
        for text in [
            "https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/debian-13.7.0-amd64-netinst.iso",
            "  http://a.example/archive.TAR.GZ?token=1#x \n",
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Debian",
        ] {
            assert_eq!(candidate(text).as_deref(), Some(text.trim()), "{text}");
        }
        for text in [
            "",
            "https://a.example/",
            "https://a.example/index.html",
            "https://a.example/forum/viewtopic.php?t=3",
            "https://a.example/dossier",
            "https://a.example/.profile",
            "voir https://a.example/f.iso",
            "https://a.example/f.iso https://a.example/g.iso",
            "ftp://a.example/f.iso",
            "file:///C:/f.iso",
            "magnet:?dn=sans-empreinte",
            "C:\\Users\\moi\\f.iso",
            "un mot de passe très secret",
        ] {
            assert_eq!(candidate(text), None, "{text}");
        }
        assert_eq!(
            candidate(&format!("https://a.example/{}.iso", "x".repeat(MAX_TEXT))),
            None
        );
    }

    #[test]
    fn the_shown_name_is_decoded_and_short() {
        assert_eq!(
            shown_name("https://a.example/t%C3%A9l%C3%A9/b%C3%A9b%C3%A9.iso?x=1"),
            "bébé.iso"
        );
        assert_eq!(
            shown_name("magnet:?xt=urn:btih:abc&dn=Debian+13"),
            "Debian 13"
        );
        assert_eq!(shown_name("magnet:?xt=urn:btih:abc"), "magnet");
        let long = shown_name(&format!("https://a.example/{}.iso", "a".repeat(200)));
        assert_eq!(long.chars().count(), 81);
        assert!(long.ends_with('…'));
        assert_eq!(percent_decode("a%2"), "a%2");
    }

    #[test]
    fn texts_follow_the_interface_language() {
        assert_eq!(
            texts("fr", "f.iso").1,
            "f.iso : cliquez pour l'ajouter à AuroDownloader"
        );
        assert_eq!(texts("en", "f.iso").0, "Link copied");
    }
}
