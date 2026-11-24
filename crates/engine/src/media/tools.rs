//! Outils externes de la vidéo (SPEC §7.17) : yt-dlp, ffmpeg et ffprobe.
//!
//! Rien ne s'installe sans accord : [`Tools::install`] n'est appelé que sur la demande de
//! l'utilisateur (`media.installTools`). Chaque fichier téléchargé est vérifié avant de
//! servir : signature du projet sur la liste des empreintes, puis empreinte du fichier pour
//! yt-dlp ; empreinte publiée seulement pour ffmpeg, que BtbN ne signe pas. Il s'écrit sous
//! un nom provisoire, se vide sur disque, puis prend son nom.
//!
//! L'état des outils installés est gardé dans `tools.json`, dans le même dossier : version,
//! empreinte et date de la dernière recherche de mise à jour de yt-dlp. Les empreintes de
//! yt-dlp, de ffmpeg et de ffprobe sont revérifiées une fois par processus, avant le
//! premier usage (audit V24, [`Tools::verify_once`]).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::sync::{RwLock, RwLockReadGuard};

use super::openpgp::{self, PgpError, PublicKey};
use crate::net::{Client, NetError};

/// Clé de signature des publications de yt-dlp : `public.key` de son dépôt.
const YTDLP_KEY: &str = include_str!("yt-dlp-public.key");

/// Empreinte de cette clé, relevée le 2026-11-09 (`gpg --fingerprint`) ; la clé lue doit
/// l'avoir.
pub const YTDLP_FINGERPRINT: [u8; 20] = [
    0xAC, 0x0C, 0xBB, 0xE6, 0x84, 0x8D, 0x6A, 0x87, 0x34, 0x64, 0xAF, 0x4E, 0x57, 0xCF, 0x65, 0x93,
    0x3B, 0x5A, 0x75, 0x81,
];

/// Branche stable de ffmpeg retenue dans les builds BtbN.
pub const FFMPEG_BRANCH: &str = "8.1";

/// Au plus une recherche de mise à jour de yt-dlp par 24 heures (SPEC §7.17).
pub const UPDATE_EVERY_MS: i64 = 24 * 3600 * 1000;

/// Taille maximale d'un outil ou d'une archive téléchargés : un garde-fou.
const MAX_DOWNLOAD: u64 = 1 << 30;

/// Taille maximale d'une liste d'empreintes, d'une signature ou d'une réponse de l'API.
const MAX_SMALL: usize = 1 << 20;

/// Fichier d'état, dans le dossier des outils.
const STATE_FILE: &str = "tools.json";

/// D'où viennent les outils.
#[derive(Debug, Clone)]
pub struct Sources {
    /// Dernière publication de yt-dlp (API de GitHub, JSON avec `tag_name`).
    pub ytdlp_latest: String,
    /// Fichiers d'une publication de yt-dlp : `{ytdlp_download}{version}/{fichier}`.
    pub ytdlp_download: String,
    /// Clé qui signe `SHA2-256SUMS`.
    pub ytdlp_key: PublicKey,
    /// Fichiers des builds de ffmpeg : `{ffmpeg_download}{fichier}`.
    pub ffmpeg_download: String,
    /// Sous Linux, prendre d'abord le ffmpeg du système (`PATH`).
    pub system_ffmpeg: bool,
    /// Essais : un yt-dlp déjà là (faux yt-dlp des tests), pris tel quel, sans installation
    /// ni mise à jour.
    pub local_ytdlp: Option<PathBuf>,
    /// Essais : ffmpeg et ffprobe déjà là (faux outils des tests), pris tels quels.
    pub local_ffmpeg: Option<(PathBuf, PathBuf)>,
}

impl Default for Sources {
    fn default() -> Self {
        Sources {
            ytdlp_latest: "https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest".into(),
            ytdlp_download: "https://github.com/yt-dlp/yt-dlp/releases/download/".into(),
            ytdlp_key: ytdlp_key(),
            ffmpeg_download: "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/"
                .into(),
            system_ffmpeg: cfg!(target_os = "linux"),
            local_ytdlp: None,
            local_ffmpeg: None,
        }
    }
}

/// La clé de yt-dlp intégrée au moteur ; une clé abîmée est une erreur de construction.
pub fn ytdlp_key() -> PublicKey {
    let key = openpgp::parse_armored_key(YTDLP_KEY).expect("clé de yt-dlp lisible");
    assert_eq!(
        key.fingerprint, YTDLP_FINGERPRINT,
        "clé de yt-dlp inattendue"
    );
    key
}

/// Origine d'un outil.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Téléchargé et vérifié par le moteur.
    Download,
    /// Celui du système (`PATH`).
    System,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Download => "download",
            Origin::System => "system",
        }
    }
}

/// Un outil prêt à servir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub version: String,
    pub origin: Origin,
    pub path: PathBuf,
}

/// État des outils (`media.tools`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    pub ytdlp: Option<Tool>,
    /// ffmpeg ; ffprobe est à côté de lui ([`Status::ffprobe`]).
    pub ffmpeg: Option<Tool>,
    pub ffprobe: Option<PathBuf>,
    /// Une installation est en cours.
    pub installing: bool,
    /// Échec de la dernière installation ou mise à jour.
    pub error: Option<String>,
}

impl Status {
    /// Tout ce qu'il faut à une vidéo est là.
    pub fn ready(&self) -> bool {
        self.ytdlp.is_some() && self.ffmpeg.is_some() && self.ffprobe.is_some()
    }
}

/// Erreurs d'installation ou de mise à jour.
#[derive(Debug, thiserror::Error)]
pub enum ToolsError {
    #[error("réseau : {0}")]
    Net(#[from] NetError),
    #[error("disque : {0}")]
    Disk(#[from] std::io::Error),
    #[error("disque : {0}")]
    Io(#[from] crate::io::IoError),
    #[error("signature de la liste des empreintes refusée : {0}")]
    Signature(#[from] PgpError),
    #[error("{0}")]
    Refused(String),
    #[error("plateforme sans vidéo (Windows et Linux seulement)")]
    Unsupported,
}

/// Ce qui est installé, tel que `tools.json` le garde.
#[derive(Debug, Clone, Default)]
struct Recorded {
    /// Version, et empreinte SHA-256 du programme.
    ytdlp: Option<(String, String)>,
    /// Version, et empreinte SHA-256 de l'archive publiée.
    ffmpeg: Option<(String, String)>,
    /// Empreintes SHA-256 de ffmpeg et de ffprobe tirés de l'archive ; absentes d'un
    /// `tools.json` d'avant le 2026-11-24, relevées alors à la première vérification.
    ffmpeg_files: Option<(String, String)>,
    last_check_ms: Option<i64>,
}

#[derive(Default)]
struct Live {
    recorded: Recorded,
    installing: bool,
    error: Option<String>,
    /// ffmpeg du système, cherché une fois par processus.
    system: Option<Option<Tool>>,
    /// Outils téléchargés revérifiés par ce processus (audit V24).
    verified: bool,
}

/// Les outils d'un moteur.
pub struct Tools {
    dir: PathBuf,
    sources: Sources,
    client: Client,
    live: Mutex<Live>,
    /// Exécutions de yt-dlp en cours (lecture) : une mise à jour ne remplace le fichier
    /// qu'entre deux (écriture).
    in_use: RwLock<()>,
    /// Une installation ou une mise à jour à la fois.
    busy: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for Tools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tools").field("dir", &self.dir).finish()
    }
}

/// Nom de yt-dlp publié pour cette plateforme.
pub(crate) fn ytdlp_asset() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("yt-dlp.exe"),
        ("windows", "aarch64") => Some("yt-dlp_arm64.exe"),
        ("linux", "x86_64") => Some("yt-dlp_linux"),
        ("linux", "aarch64") => Some("yt-dlp_linux_aarch64"),
        _ => None,
    }
}

/// Archive de ffmpeg publiée par BtbN pour cette plateforme.
pub(crate) fn ffmpeg_asset() -> Option<String> {
    let (os, ext) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => ("win64", "zip"),
        ("windows", "aarch64") => ("winarm64", "zip"),
        ("linux", "x86_64") => ("linux64", "tar.xz"),
        ("linux", "aarch64") => ("linuxarm64", "tar.xz"),
        _ => return None,
    };
    Some(format!(
        "ffmpeg-n{FFMPEG_BRANCH}-latest-{os}-gpl-{FFMPEG_BRANCH}.{ext}"
    ))
}

/// Nom d'un exécutable sur cette plateforme.
pub(crate) fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

impl Tools {
    /// Les outils du dossier `dir`, d'après son `tools.json` (absent : aucun). Un outil
    /// enregistré dont le fichier manque est oublié.
    pub fn open(dir: PathBuf, sources: Sources, client: Client) -> Tools {
        let recorded = read_state(&dir.join(STATE_FILE)).unwrap_or_default();
        let tools = Tools {
            dir,
            sources,
            client,
            live: Mutex::new(Live {
                recorded,
                ..Live::default()
            }),
            in_use: RwLock::new(()),
            busy: tokio::sync::Mutex::new(()),
        };
        {
            let mut live = tools.live.lock().unwrap();
            if !tools.ytdlp_path().is_file() {
                live.recorded.ytdlp = None;
            }
            if !(tools.ffmpeg_path().is_file() && tools.ffprobe_path().is_file()) {
                live.recorded.ffmpeg = None;
                live.recorded.ffmpeg_files = None;
            }
        }
        tools
    }

    fn ytdlp_path(&self) -> PathBuf {
        self.dir.join(exe("yt-dlp"))
    }

    fn ffmpeg_path(&self) -> PathBuf {
        self.dir.join(exe("ffmpeg"))
    }

    fn ffprobe_path(&self) -> PathBuf {
        self.dir.join(exe("ffprobe"))
    }

    /// État des outils.
    pub fn status(&self) -> Status {
        let system = self.system_ffmpeg();
        let live = self.live.lock().unwrap();
        let ytdlp = match &self.sources.local_ytdlp {
            Some(path) => Some(Tool {
                version: "local".into(),
                origin: Origin::System,
                path: path.clone(),
            }),
            None => live.recorded.ytdlp.as_ref().map(|(version, _)| Tool {
                version: version.clone(),
                origin: Origin::Download,
                path: self.ytdlp_path(),
            }),
        };
        let local = self.sources.local_ffmpeg.clone().map(|(ffmpeg, ffprobe)| {
            (
                Tool {
                    version: "local".into(),
                    origin: Origin::System,
                    path: ffmpeg,
                },
                ffprobe,
            )
        });
        let (ffmpeg, ffprobe) = match (&live.recorded.ffmpeg, system) {
            _ if local.is_some() => {
                let (tool, probe) = local.expect("présent");
                (Some(tool), Some(probe))
            }
            (Some((version, _)), _) => (
                Some(Tool {
                    version: version.clone(),
                    origin: Origin::Download,
                    path: self.ffmpeg_path(),
                }),
                Some(self.ffprobe_path()),
            ),
            (None, Some(tool)) => {
                let probe = tool.path.with_file_name(exe("ffprobe"));
                (Some(tool), Some(probe))
            }
            (None, None) => (None, None),
        };
        Status {
            ytdlp,
            ffmpeg,
            ffprobe,
            installing: live.installing,
            error: live.error.clone(),
        }
    }

    /// ffmpeg et ffprobe du système, cherchés une fois (Linux, [`Sources::system_ffmpeg`]).
    fn system_ffmpeg(&self) -> Option<Tool> {
        if !self.sources.system_ffmpeg {
            return None;
        }
        if let Some(found) = &self.live.lock().unwrap().system {
            return found.clone();
        }
        let found = find_in_path(&exe("ffmpeg"))
            .filter(|f| f.with_file_name(exe("ffprobe")).is_file())
            .map(|path| Tool {
                version: system_version(&path).unwrap_or_else(|| "?".into()),
                origin: Origin::System,
                path,
            });
        self.live.lock().unwrap().system = Some(found.clone());
        found
    }

    /// Marque une installation comme commencée ; `false` si une autre est en cours.
    /// L'appelant lance ensuite [`Tools::install`].
    pub fn begin_install(&self) -> bool {
        let mut live = self.live.lock().unwrap();
        if live.installing {
            return false;
        }
        live.installing = true;
        live.error = None;
        true
    }

    /// Installe ce qui manque (tout si `force`) : yt-dlp à sa dernière version, ffmpeg de
    /// la branche [`FFMPEG_BRANCH`] (sous Linux, seulement si le système n'en a pas).
    /// Seulement sur la demande de l'utilisateur (SPEC §7.17).
    pub async fn install(&self, force: bool) -> Result<Status, ToolsError> {
        let result = self.install_inner(force).await;
        {
            let mut live = self.live.lock().unwrap();
            live.installing = false;
            live.error = result.as_ref().err().map(|e| e.to_string());
        }
        result.map(|()| self.status())
    }

    async fn install_inner(&self, force: bool) -> Result<(), ToolsError> {
        let _busy = self.busy.lock().await;
        if ytdlp_asset().is_none() {
            return Err(ToolsError::Unsupported);
        }
        let status = self.status();
        if force || status.ytdlp.is_none() {
            let version = self.latest_ytdlp().await?;
            self.install_ytdlp(&version).await?;
            self.live.lock().unwrap().recorded.last_check_ms = Some(crate::store::now_ms());
            self.save()?;
        }
        let downloaded = matches!(
            status.ffmpeg,
            Some(Tool {
                origin: Origin::Download,
                ..
            })
        );
        if status.ffmpeg.is_none() || (force && downloaded) {
            self.install_ffmpeg().await?;
        }
        Ok(())
    }

    /// Met yt-dlp à jour si c'est le moment (au plus une fois par [`UPDATE_EVERY_MS`]) et
    /// qu'une version plus récente est publiée ; rend la nouvelle version. yt-dlp doit
    /// être installé ; le réglage `media.autoUpdate` est vérifié par l'appelant.
    pub async fn update_ytdlp_if_due(&self, now_ms: i64) -> Result<Option<String>, ToolsError> {
        let Ok(_busy) = self.busy.try_lock() else {
            return Ok(None);
        };
        let installed = {
            let live = self.live.lock().unwrap();
            let due = live
                .recorded
                .last_check_ms
                .is_none_or(|last| now_ms - last >= UPDATE_EVERY_MS || now_ms < last);
            match &live.recorded.ytdlp {
                Some((version, _)) if due => version.clone(),
                _ => return Ok(None),
            }
        };
        self.live.lock().unwrap().recorded.last_check_ms = Some(now_ms);
        self.save()?;
        let latest = self.latest_ytdlp().await?;
        // Seulement vers une version plus récente : une publication plus ancienne, même
        // signée, ramènerait des failles corrigées depuis (audit V10, décision 11).
        if !is_newer(&latest, &installed) {
            if latest != installed {
                tracing::warn!("yt-dlp : la publication annoncée n'est pas plus récente, gardé");
            }
            return Ok(None);
        }
        let result = self.install_ytdlp(&latest).await;
        self.live.lock().unwrap().error = result.as_ref().err().map(|e| e.to_string());
        result.map(|()| Some(latest))
    }

    /// yt-dlp gardé pendant toute l'exécution d'une vidéo : une mise à jour attend qu'elle
    /// finisse, au lieu de remplacer le programme entre la résolution et les flux (audit V8).
    pub async fn hold(&self) -> RwLockReadGuard<'_, ()> {
        self.in_use.read().await
    }

    /// yt-dlp, gardé tant que le garde vit : une mise à jour attend qu'il soit rendu.
    pub async fn ytdlp(&self) -> Option<(PathBuf, RwLockReadGuard<'_, ()>)> {
        let guard = self.in_use.read().await;
        self.status().ytdlp.map(|t| (t.path, guard))
    }

    /// Revérifie les outils téléchargés, une fois par processus, avant leur premier usage
    /// (SPEC §7.17, audit V24) : un outil dont le fichier n'a plus l'empreinte enregistrée
    /// (abîmé, remplacé) est oublié, comme un fichier manquant, et l'erreur le dit ; la
    /// vidéo demande alors de le réinstaller. Les outils du système et les outils imposés
    /// ne sont pas vérifiés. Pendant une installation ou une mise à jour, rien n'est fait :
    /// la vérification se refait à l'usage suivant.
    pub async fn verify_once(&self) {
        let Ok(_busy) = self.busy.try_lock() else {
            return;
        };
        let (ytdlp, ffmpeg, files) = {
            let live = self.live.lock().unwrap();
            if live.verified {
                return;
            }
            (
                live.recorded.ytdlp.as_ref().map(|(_, sum)| sum.clone()),
                live.recorded.ffmpeg.is_some(),
                live.recorded.ffmpeg_files.clone(),
            )
        };
        let paths = (self.ytdlp_path(), self.ffmpeg_path(), self.ffprobe_path());
        let sums = tokio::task::spawn_blocking(move || {
            let ytdlp = ytdlp.map(|_| sha256_of(&paths.0));
            let ffmpeg = ffmpeg.then(|| (sha256_of(&paths.1), sha256_of(&paths.2)));
            (ytdlp, ffmpeg)
        })
        .await;
        let Ok((ytdlp_now, ffmpeg_now)) = sums else {
            return;
        };
        let mut changed = false;
        {
            let mut live = self.live.lock().unwrap();
            let mut damaged = Vec::new();
            if let (Some(now), Some((_, sum))) = (&ytdlp_now, &live.recorded.ytdlp)
                && now.as_deref() != Some(sum.as_str())
            {
                live.recorded.ytdlp = None;
                damaged.push("yt-dlp");
            }
            if let Some((Some(ffmpeg_now), Some(ffprobe_now))) = ffmpeg_now {
                match files {
                    Some((ffmpeg_sum, ffprobe_sum)) => {
                        if ffmpeg_now != ffmpeg_sum || ffprobe_now != ffprobe_sum {
                            live.recorded.ffmpeg = None;
                            live.recorded.ffmpeg_files = None;
                            damaged.push("ffmpeg");
                        }
                    }
                    // `tools.json` d'avant le relevé des fichiers : relevés maintenant.
                    None => {
                        live.recorded.ffmpeg_files = Some((ffmpeg_now, ffprobe_now));
                        changed = true;
                    }
                }
            } else if ffmpeg {
                live.recorded.ffmpeg = None;
                live.recorded.ffmpeg_files = None;
                damaged.push("ffmpeg");
            }
            if !damaged.is_empty() {
                tracing::warn!(outils = ?damaged, "outils de la vidéo modifiés depuis leur installation, oubliés");
                live.error = Some(format!(
                    "{} modifié depuis son installation : à réinstaller",
                    damaged.join(" et ")
                ));
                changed = true;
            }
            live.verified = true;
        }
        if changed && let Err(e) = self.save() {
            tracing::warn!(erreur = %e, "état des outils de la vidéo non enregistré");
        }
    }

    /// Version de la dernière publication de yt-dlp.
    async fn latest_ytdlp(&self) -> Result<String, ToolsError> {
        let body = self.fetch(&self.sources.ytdlp_latest).await?;
        let json: Value = serde_json::from_slice(&body)
            .map_err(|_| ToolsError::Refused("réponse illisible de la publication".into()))?;
        let tag = json["tag_name"]
            .as_str()
            .filter(|t| {
                !t.is_empty()
                    && t.len() <= 64
                    && t.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            })
            .ok_or_else(|| ToolsError::Refused("version de yt-dlp illisible".into()))?;
        Ok(tag.to_owned())
    }

    /// Télécharge et vérifie yt-dlp `version`, puis le met en place entre deux exécutions.
    async fn install_ytdlp(&self, version: &str) -> Result<(), ToolsError> {
        let asset = ytdlp_asset().ok_or(ToolsError::Unsupported)?;
        let base = format!("{}{version}/", self.sources.ytdlp_download);
        let sums = self.fetch(&format!("{base}SHA2-256SUMS")).await?;
        let signature = self.fetch(&format!("{base}SHA2-256SUMS.sig")).await?;
        openpgp::verify_detached(&self.sources.ytdlp_key, &sums, &signature)?;
        let expected = find_sum(&sums, asset)?;
        std::fs::create_dir_all(&self.dir)?;
        let partial = self.dir.join(".yt-dlp.download");
        self.download_verified(&format!("{base}{asset}"), &expected, &partial)
            .await?;
        make_executable(&partial)?;
        {
            // Pas pendant qu'un yt-dlp tourne (sous Windows, son fichier est d'ailleurs
            // verrouillé).
            let _replace = self.in_use.write().await;
            std::fs::rename(&partial, self.ytdlp_path())?;
            crate::io::sync_parent_dir(&self.ytdlp_path())?;
            self.live.lock().unwrap().recorded.ytdlp = Some((version.to_owned(), expected));
        }
        self.save()
    }

    /// Télécharge l'archive de ffmpeg, vérifie son empreinte et en tire ffmpeg et ffprobe.
    async fn install_ffmpeg(&self) -> Result<(), ToolsError> {
        let asset = ffmpeg_asset().ok_or(ToolsError::Unsupported)?;
        let sums = self
            .fetch(&format!("{}checksums.sha256", self.sources.ffmpeg_download))
            .await?;
        let expected = find_sum(&sums, &asset)?;
        std::fs::create_dir_all(&self.dir)?;
        let archive = self.dir.join(".ffmpeg.download");
        self.download_verified(
            &format!("{}{asset}", self.sources.ffmpeg_download),
            &expected,
            &archive,
        )
        .await?;
        let dir = self.dir.clone();
        let archive_for_task = archive.clone();
        let extracted =
            tokio::task::spawn_blocking(move || extract_ffmpeg(&archive_for_task, &dir))
                .await
                .map_err(|e| ToolsError::Refused(format!("extraction interrompue : {e}")))?;
        let _ = std::fs::remove_file(&archive);
        extracted?;
        for name in ["ffmpeg", "ffprobe"] {
            let target = self.dir.join(exe(name));
            std::fs::rename(self.dir.join(format!(".{}.new", exe(name))), &target)?;
        }
        crate::io::sync_parent_dir(&self.ffmpeg_path())?;
        let (ffmpeg, ffprobe) = (self.ffmpeg_path(), self.ffprobe_path());
        let files = tokio::task::spawn_blocking(move || (sha256_of(&ffmpeg), sha256_of(&ffprobe)))
            .await
            .map_err(|e| ToolsError::Refused(format!("empreinte interrompue : {e}")))?;
        let (Some(ffmpeg_sum), Some(ffprobe_sum)) = files else {
            return Err(ToolsError::Refused(
                "ffmpeg ou ffprobe illisible après l'extraction".into(),
            ));
        };
        {
            let mut live = self.live.lock().unwrap();
            live.recorded.ffmpeg = Some((format!("{FFMPEG_BRANCH} (BtbN)"), expected));
            live.recorded.ffmpeg_files = Some((ffmpeg_sum, ffprobe_sum));
        }
        self.save()
    }

    /// Un petit fichier (liste d'empreintes, signature, réponse de l'API).
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, ToolsError> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| ToolsError::Refused("adresse de publication invalide".into()))?;
        self.client
            .fetch_small(&parsed, MAX_SMALL)
            .await?
            .ok_or_else(|| {
                ToolsError::Refused(format!(
                    "fichier de publication absent : {}",
                    file_part(url)
                ))
            })
    }

    /// Télécharge `url` dans `path` en calculant son empreinte ; la refuse (et efface le
    /// fichier) si elle diffère de `expected` ; sinon le vide sur disque.
    async fn download_verified(
        &self,
        url: &str,
        expected: &str,
        path: &Path,
    ) -> Result<(), ToolsError> {
        let result = async {
            let mut body = self.client.get_all(url, &[], None).await?;
            let mut file = std::fs::File::create(path)?;
            let mut hasher = Sha256::new();
            let mut total = 0u64;
            while let Some(chunk) = body.chunk().await? {
                total += chunk.len() as u64;
                if total > MAX_DOWNLOAD {
                    return Err(ToolsError::Refused(format!(
                        "{} : fichier trop gros",
                        file_part(url)
                    )));
                }
                hasher.update(&chunk);
                file.write_all(&chunk)?;
            }
            let got = hex(&hasher.finalize());
            if got != expected {
                return Err(ToolsError::Refused(format!(
                    "{} : empreinte différente de celle publiée",
                    file_part(url)
                )));
            }
            file.sync_all()?;
            Ok(())
        }
        .await;
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result
    }

    /// Enregistre `tools.json` : fichier provisoire vidé, puis renommé.
    fn save(&self) -> Result<(), ToolsError> {
        let recorded = self.live.lock().unwrap().recorded.clone();
        let tool = |t: &Option<(String, String)>| {
            t.as_ref()
                .map(|(version, sha256)| json!({"version": version, "sha256": sha256}))
        };
        let mut ffmpeg = tool(&recorded.ffmpeg);
        if let (Some(ffmpeg), Some((ffmpeg_sum, ffprobe_sum))) =
            (ffmpeg.as_mut(), &recorded.ffmpeg_files)
        {
            ffmpeg["files"] = json!({"ffmpeg": ffmpeg_sum, "ffprobe": ffprobe_sum});
        }
        let text = json!({
            "ytdlp": tool(&recorded.ytdlp),
            "ffmpeg": ffmpeg,
            "lastCheckMs": recorded.last_check_ms,
        })
        .to_string();
        std::fs::create_dir_all(&self.dir)?;
        let partial = self.dir.join(format!(".{STATE_FILE}.new"));
        let mut file = std::fs::File::create(&partial)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        let path = self.dir.join(STATE_FILE);
        std::fs::rename(&partial, &path)?;
        crate::io::sync_parent_dir(&path)?;
        Ok(())
    }
}

fn read_state(path: &Path) -> Option<Recorded> {
    let json: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let tool = |v: &Value| {
        Some((
            v["version"].as_str()?.to_owned(),
            v["sha256"].as_str()?.to_owned(),
        ))
    };
    let files = &json["ffmpeg"]["files"];
    Some(Recorded {
        ytdlp: tool(&json["ytdlp"]),
        ffmpeg: tool(&json["ffmpeg"]),
        ffmpeg_files: files["ffmpeg"]
            .as_str()
            .zip(files["ffprobe"].as_str())
            .map(|(a, b)| (a.to_owned(), b.to_owned())),
        last_check_ms: json["lastCheckMs"].as_i64(),
    })
}

/// Empreinte SHA-256 du fichier `path`, en hexadécimal minuscule ; `None` s'il ne se lit
/// pas.
fn sha256_of(path: &Path) -> Option<String> {
    crate::io::checksum::hash_file(path, crate::io::checksum::Algo::Sha256, |_| true)
        .ok()
        .flatten()
}

/// L'empreinte de `name` dans une liste au format de `sha256sum` (`<hex>  <nom>`, ou
/// `<hex> *<nom>`).
fn find_sum(list: &[u8], name: &str) -> Result<String, ToolsError> {
    let text = std::str::from_utf8(list)
        .map_err(|_| ToolsError::Refused("liste des empreintes illisible".into()))?;
    text.lines()
        .filter_map(|line| {
            let (sum, file) = line.trim_end().split_once(char::is_whitespace)?;
            let file = file.trim_start().trim_start_matches('*');
            (file == name && sum.len() == 64 && sum.chars().all(|c| c.is_ascii_hexdigit()))
                .then(|| sum.to_ascii_lowercase())
        })
        .next()
        .ok_or_else(|| ToolsError::Refused(format!("{name} absent de la liste des empreintes")))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Le nom de fichier d'une adresse, seul cité dans les messages.
fn file_part(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

fn make_executable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    let _ = path;
    Ok(())
}

/// Un exécutable dans le `PATH`.
fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// Version d'un ffmpeg du système (`ffmpeg version 6.1.1-3ubuntu5 …`).
fn system_version(path: &Path) -> Option<String> {
    let output = std::process::Command::new(path)
        .arg("-version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?;
    line.strip_prefix("ffmpeg version ")?
        .split_whitespace()
        .next()
        .map(str::to_owned)
}

/// Tire ffmpeg et ffprobe de l'archive (ils sont dans son dossier `bin/`) vers
/// `.ffmpeg.new` et `.ffprobe.new`, vidés sur disque.
fn extract_ffmpeg(archive: &Path, dir: &Path) -> Result<(), ToolsError> {
    let wanted = [exe("ffmpeg"), exe("ffprobe")];
    let mut found = [false, false];
    let mut write = |name: &str, reader: &mut dyn std::io::Read| -> Result<bool, ToolsError> {
        let Some(i) = wanted
            .iter()
            .position(|w| name.ends_with(&format!("/bin/{w}")))
        else {
            return Ok(false);
        };
        let path = dir.join(format!(".{}.new", wanted[i]));
        let mut file = std::fs::File::create(&path)?;
        std::io::copy(
            &mut std::io::Read::take(&mut *reader, MAX_DOWNLOAD),
            &mut file,
        )?;
        file.sync_all()?;
        drop(file);
        make_executable(&path)?;
        found[i] = true;
        Ok(true)
    };
    for_each_entry(archive, &mut write)?;
    if found != [true, true] {
        return Err(ToolsError::Refused(
            "archive de ffmpeg sans ffmpeg ou ffprobe".into(),
        ));
    }
    Ok(())
}

/// Chaque fichier d'une archive de BtbN, par son chemin (séparé par `/`) : zip sous
/// Windows.
#[cfg(windows)]
fn for_each_entry(
    archive: &Path,
    each: &mut dyn FnMut(&str, &mut dyn std::io::Read) -> Result<bool, ToolsError>,
) -> Result<(), ToolsError> {
    let unreadable = |e: zip::result::ZipError| {
        ToolsError::Refused(format!("archive de ffmpeg illisible : {e}"))
    };
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?).map_err(unreadable)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(unreadable)?;
        let name = entry.name().replace('\\', "/");
        each(&name, &mut entry)?;
    }
    Ok(())
}

/// Chaque fichier d'une archive de BtbN : `.tar.xz` sous Linux.
#[cfg(target_os = "linux")]
fn for_each_entry(
    archive: &Path,
    each: &mut dyn FnMut(&str, &mut dyn std::io::Read) -> Result<bool, ToolsError>,
) -> Result<(), ToolsError> {
    let xz = xz2::read::XzDecoder::new(std::fs::File::open(archive)?);
    let mut tar = tar::Archive::new(xz);
    for entry in tar.entries()? {
        let mut entry = entry?;
        let name = entry.path()?.to_string_lossy().into_owned();
        each(&name, &mut entry)?;
    }
    Ok(())
}

/// Ailleurs, pas de vidéo en v1.
#[cfg(not(any(windows, target_os = "linux")))]
fn for_each_entry(
    _archive: &Path,
    _each: &mut dyn FnMut(&str, &mut dyn std::io::Read) -> Result<bool, ToolsError>,
) -> Result<(), ToolsError> {
    Err(ToolsError::Unsupported)
}

/// `latest` est-elle une version de yt-dlp plus récente qu'`installed` ? Les étiquettes
/// sont des dates (`2025.10.22`, parfois `2025.10.22.1`), comparées nombre par nombre ;
/// une étiquette illisible n'est jamais plus récente.
fn is_newer(latest: &str, installed: &str) -> bool {
    let parse = |tag: &str| {
        tag.split('.')
            .map(|part| part.parse::<u64>().ok())
            .collect::<Option<Vec<u64>>>()
    };
    match (parse(latest), parse(installed)) {
        (Some(latest), Some(installed)) => latest > installed,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
