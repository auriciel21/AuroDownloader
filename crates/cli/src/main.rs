//! `auro` : ligne de commande d'AuroDownloader (F-40).
//!
//! La CLI est un client du moteur `aurod`, qu'elle joint par le protocole JSON-RPC sur le
//! point d'accès local (SPEC §6) et qu'elle démarre s'il ne tourne pas. Commandes : `add`,
//! `get` (ajoute et suit jusqu'à la fin), `list`, `pause`, `resume`, `cancel`, `remove`,
//! `watch`, `files` (fichiers d'un torrent), `limit` (limites de débit), `config`
//! (réglages), `video` (yt-dlp et ffmpeg), `queue`, `engine`. Sans moteur : `probe` prépare
//! une tâche sans rien télécharger (SPEC §7.1), `db migrate` crée ou met à jour une base,
//! et `direct` exécute une tâche avec la bibliothèque du moteur (bancs, dépannage).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use auro_engine::io::checksum::Checksum;
use auro_engine::net::Client;
use auro_engine::paths;
use clap::{Parser, Subcommand};
use serde_json::json;

mod direct;
mod display;
mod links;
mod remote;
mod video;

use auro_cli::client;

use client::{Rpc, Target};
use display::Limit;
use remote::AddOptions;

#[derive(Parser)]
#[command(name = "auro", version, about = "AuroDownloader : ligne de commande")]
struct Cli {
    /// Point d'accès du moteur : canal nommé (Windows) ou socket (Unix)
    #[arg(long, global = true, value_name = "NOM")]
    endpoint: Option<String>,

    /// Ne pas démarrer le moteur s'il ne tourne pas
    #[arg(long, global = true)]
    no_start: bool,

    /// Autorités de certification à ajouter à celles du système (fichier PEM) : pour
    /// `probe`, `direct` et le moteur démarré par cette commande. La validation des
    /// certificats reste stricte.
    #[arg(long, global = true, value_name = "FICHIER")]
    ca_cert: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

/// Connexions : de 1 à 32.
fn connections() -> clap::builder::RangedI64ValueParser<u32> {
    clap::value_parser!(u32).range(1..=32)
}

#[derive(Subcommand)]
enum Command {
    /// Ajoute des téléchargements au moteur, sans attendre
    Add {
        /// Adresses HTTP ou HTTPS, liens magnet, fichiers .torrent, ou fichiers Metalink
        /// (.meta4, .metalink : une tâche par fichier décrit)
        #[arg(required = true)]
        urls: Vec<String>,
        /// Dossier de destination
        #[arg(short = 'o', long, default_value = ".")]
        dir: PathBuf,
        /// Dossier des téléchargements du moteur au lieu de -o : les règles de rangement
        /// y choisissent le dossier et le nom (auro rules)
        #[arg(long, conflicts_with = "dir")]
        auto: bool,
        /// Nombre de connexions (1 à 32 ; automatique sinon)
        #[arg(short = 'c', long, value_parser = connections())]
        connections: Option<u32>,
        /// File (nom ou identifiant)
        #[arg(short = 'q', long)]
        queue: Option<String>,
        /// Limite de débit de chaque tâche (500K, 2M…)
        #[arg(short = 'l', long, value_name = "DÉBIT", value_parser = display::parse_limit)]
        limit: Option<Limit>,
        /// Ajouter en pause
        #[arg(long)]
        paused: bool,
        /// En-tête HTTP « Nom: valeur », à répéter (Referer, Cookie…). Cookie et
        /// Authorization vont au coffre du système, mais restent visibles dans
        /// l'historique du terminal
        #[arg(short = 'H', long = "header", value_name = "EN-TÊTE", value_parser = display::parse_header)]
        headers: Vec<(String, String)>,
        /// Empreinte attendue du fichier (une seule adresse), vérifiée avant qu'il prenne
        /// son nom : « sha256:… », « sha1:… », « md5:… », ou la valeur seule
        #[arg(long, value_name = "EMPREINTE", value_parser = display::parse_checksum)]
        checksum: Option<Checksum>,
        /// Miroir du même fichier (une seule adresse), à répéter : le moteur vérifie qu'il
        /// sert bien les mêmes octets, puis répartit le téléchargement
        #[arg(short = 'm', long = "mirror", value_name = "ADRESSE")]
        mirrors: Vec<String>,
        /// Fichiers d'un Metalink à ajouter : « 0,2-4 » (rangs montrés par auro metalink) ;
        /// tous sinon
        #[arg(long, value_name = "RANGS", value_parser = parse_selection)]
        select: Option<Selection>,
        /// L'adresse telle quelle, même si un plugin prend son site en charge
        #[arg(long)]
        no_plugin: bool,
    },
    /// Télécharge un fichier et suit l'avancement (Ctrl-C met la tâche en pause)
    Get {
        /// Adresse HTTP ou HTTPS, lien magnet, fichier .torrent, ou fichier Metalink
        url: String,
        /// Dossier de destination
        #[arg(short = 'o', long, default_value = ".")]
        dir: PathBuf,
        /// Dossier des téléchargements du moteur au lieu de -o : les règles de rangement
        /// y choisissent le dossier et le nom (auro rules)
        #[arg(long, conflicts_with = "dir")]
        auto: bool,
        /// Nombre de connexions (1 à 32 ; automatique sinon)
        #[arg(short = 'c', long, value_parser = connections())]
        connections: Option<u32>,
        /// File (nom ou identifiant)
        #[arg(short = 'q', long)]
        queue: Option<String>,
        /// Limite de débit de la tâche (500K, 2M…)
        #[arg(short = 'l', long, value_name = "DÉBIT", value_parser = display::parse_limit)]
        limit: Option<Limit>,
        /// En-tête HTTP « Nom: valeur », à répéter (voir `add`)
        #[arg(short = 'H', long = "header", value_name = "EN-TÊTE", value_parser = display::parse_header)]
        headers: Vec<(String, String)>,
        /// Empreinte attendue du fichier (voir `add`)
        #[arg(long, value_name = "EMPREINTE", value_parser = display::parse_checksum)]
        checksum: Option<Checksum>,
        /// Miroir du même fichier, à répéter (voir `add`)
        #[arg(short = 'm', long = "mirror", value_name = "ADRESSE")]
        mirrors: Vec<String>,
        /// Fichiers d'un Metalink à ajouter : « 0,2-4 » (rangs montrés par auro metalink) ;
        /// tous sinon
        #[arg(long, value_name = "RANGS", value_parser = parse_selection)]
        select: Option<Selection>,
    },
    /// Liste les tâches
    List {
        /// États à montrer (queued, active, paused, retry_wait, completed, failed…)
        #[arg(short = 's', long = "state")]
        states: Vec<String>,
        /// File (nom ou identifiant)
        #[arg(short = 'q', long)]
        queue: Option<String>,
    },
    /// Met des tâches en pause
    Pause {
        #[arg(required = true)]
        task_ids: Vec<String>,
    },
    /// Remet en file des tâches en pause ou en échec
    Resume {
        #[arg(required = true)]
        task_ids: Vec<String>,
    },
    /// Annule des tâches
    Cancel {
        #[arg(required = true)]
        task_ids: Vec<String>,
        /// Supprimer aussi les fichiers partiels
        #[arg(long)]
        delete_files: bool,
    },
    /// Retire de la liste des tâches finies (terminées, en échec ou annulées)
    Remove {
        #[arg(required = true)]
        task_ids: Vec<String>,
        /// Supprimer aussi le fichier : le fichier final d'une tâche terminée, le fichier
        /// partiel des autres
        #[arg(long)]
        delete_files: bool,
    },
    /// Retélécharge depuis le début des tâches terminées, en échec, annulées ou en pause
    Restart {
        #[arg(required = true)]
        task_ids: Vec<String>,
        /// Remplacer le fichier déjà téléchargé ; sinon il reste, et le nouveau prend un
        /// autre nom
        #[arg(long)]
        delete_files: bool,
    },
    /// Déplace ou renomme le fichier d'une tâche qui ne tourne pas (le fichier final d'une
    /// tâche terminée, le fichier partiel des autres)
    Move {
        task_id: String,
        /// Nouveau dossier (existant)
        #[arg(short = 'o', long)]
        dir: Option<PathBuf>,
        /// Nouveau nom du fichier
        #[arg(short = 'n', long)]
        name: Option<String>,
    },
    /// Remplace l'adresse d'une tâche qui ne tourne pas, par exemple un lien expiré ; pour
    /// une tâche commencée, la nouvelle adresse doit donner le même fichier
    Refresh { task_id: String, url: String },
    /// Ajoute un miroir à une tâche pas encore terminée (le même fichier ailleurs) ; sans
    /// adresse, montre ses sources : rang, état, débit, raison d'un refus
    Mirror {
        task_id: String,
        url: Option<String>,
    },
    /// Donne, change ou retire l'empreinte attendue du fichier d'une tâche pas encore
    /// terminée ; après un échec CHECKSUM_MISMATCH, `auro resume` vérifie de nouveau le
    /// fichier reçu, sans le retélécharger
    Checksum {
        task_id: String,
        /// « sha256:… », « sha1:… », « md5:… », ou la valeur seule
        #[arg(value_name = "EMPREINTE", value_parser = display::parse_checksum,
              required_unless_present = "clear")]
        checksum: Option<Checksum>,
        /// Retirer l'empreinte
        #[arg(long, conflicts_with = "checksum")]
        clear: bool,
    },
    /// Suit l'avancement des tâches (toutes celles en cours par défaut)
    Watch { task_ids: Vec<String> },
    /// Liens d'une page, d'un fichier texte ou CSV, ou de l'entrée standard (« - ») : nom,
    /// taille et adresse des fichiers trouvés ; avec --add, les ajoute
    Links {
        /// Adresse d'une page, fichier texte ou CSV, ou « - »
        source: String,
        /// Extensions à garder : « iso,zip »
        #[arg(
            short = 'e',
            long = "ext",
            value_name = "EXTENSIONS",
            value_delimiter = ','
        )]
        extensions: Vec<String>,
        /// Expression régulière que l'adresse doit contenir
        #[arg(long, value_name = "EXPRESSION")]
        regex: Option<String>,
        /// Taille minimale (500K, 2M, 1G) ; une taille inconnue écarte le lien
        #[arg(long, value_name = "TAILLE", value_parser = parse_size)]
        min_size: Option<u64>,
        /// Taille maximale ; une taille inconnue écarte le lien
        #[arg(long, value_name = "TAILLE", value_parser = parse_size)]
        max_size: Option<u64>,
        /// Montrer aussi les pages et les liens sans extension
        #[arg(long)]
        all: bool,
        /// Ne pas demander la taille des fichiers (aucune requête vers eux)
        #[arg(long, conflicts_with_all = ["min_size", "max_size"])]
        no_sizes: bool,
        /// Ajouter les liens gardés
        #[arg(long)]
        add: bool,
        /// Dossier de destination (avec --add)
        #[arg(short = 'o', long, default_value = ".", requires = "add")]
        dir: PathBuf,
        /// Ajouter en pause (avec --add)
        #[arg(long, requires = "add")]
        paused: bool,
    },
    /// Contenu d'un fichier Metalink (.meta4, .metalink ; fichier ou adresse) : fichiers,
    /// rangs, empreintes et sources, sans rien ajouter
    Metalink {
        /// Fichier local ou adresse
        source: String,
    },
    /// Fichiers d'un torrent ; avec --select, ceux à télécharger (les autres sont laissés)
    Files {
        task_id: String,
        /// Rangs des fichiers à garder : « 0,2-4 » (rangs montrés par auro files)
        #[arg(long, value_name = "RANGS", value_parser = parse_selection)]
        select: Option<Selection>,
    },
    /// Limite de débit, globale par défaut, ou d'une tâche ou d'une file ; sans débit,
    /// l'affiche. Elle s'applique aussitôt aux téléchargements en cours.
    Limit {
        /// Débit par seconde (800000, 500K, 2M, 1.5G) ou « none » pour la retirer
        #[arg(value_name = "DÉBIT", value_parser = display::parse_limit)]
        rate: Option<Limit>,
        /// Tâche dont changer la limite
        #[arg(short = 't', long, conflicts_with_all = ["queue", "upload"])]
        task: Option<String>,
        /// File (nom ou identifiant) dont changer la limite
        #[arg(short = 'q', long, conflicts_with = "upload")]
        queue: Option<String>,
        /// Limite d'envoi des torrents aux autres pairs, globale
        #[arg(long)]
        upload: bool,
    },
    /// Réglages du moteur : sans argument, tous ; avec une clé, le sien ; avec une valeur,
    /// le change (true/false, un nombre, un texte, ou « none » pour retirer la valeur).
    /// Par exemple : torrent.ratioLimit 1.5, torrent.interface none, media.autoUpdate false
    Config {
        key: Option<String>,
        value: Option<String>,
    },
    /// Vidéos de pages web, par yt-dlp et ffmpeg (sites sans DRM)
    Video {
        #[command(subcommand)]
        action: VideoAction,
    },
    /// Historique des téléchargements, les plus récents d'abord (terminés, et ceux en
    /// échec ou annulés une fois retirés de la liste)
    History {
        /// Texte cherché dans le nom, le chemin ou le site
        query: Option<String>,
        /// Au format CSV (virgules, RFC 4180), pour un tableur ou un script
        #[arg(long, conflicts_with = "clear")]
        csv: bool,
        /// Effacer tout l'historique (ni les tâches ni les fichiers)
        #[arg(long, conflicts_with = "query")]
        clear: bool,
    },
    /// Proxy des connexions HTTP(S) : celui du système (par défaut), aucun, ou un proxy
    /// choisi ; sans argument, l'affiche
    Proxy {
        #[command(subcommand)]
        action: Option<ProxyAction>,
    },
    /// Règles de rangement : dossier, nom et commande de fin des tâches ajoutées sans
    /// dossier (--auto, extension, page) ; la première règle activée qui correspond
    /// l'emporte. Sans argument, les montre
    Rules {
        #[command(subcommand)]
        action: Option<RulesAction>,
    },
    /// Diagnostic du moteur, sans adresses ni identifiants : état résumé et journaux
    /// nettoyés, montrés ; avec --save, enregistrés dans une archive .zip à joindre à un
    /// signalement
    Diagnostic {
        /// Archive à écrire (.zip), jamais par-dessus un fichier existant
        #[arg(long, value_name = "FICHIER")]
        save: Option<PathBuf>,
    },
    /// Dossier des téléchargements du moteur, pour les ajouts sans dossier (extension,
    /// application) ; sans argument, l'affiche
    Dir {
        /// Nouveau dossier (existant)
        #[arg(conflicts_with = "reset")]
        path: Option<PathBuf>,
        /// Revenir au dossier « Téléchargements » du système
        #[arg(long)]
        reset: bool,
    },
    /// Files de téléchargement
    Queue {
        #[command(subcommand)]
        action: QueueAction,
    },
    /// Identifiants de sites (Basic, Digest) : envoyés quand un serveur du site les demande ;
    /// le mot de passe va au coffre du système
    Login {
        #[command(subcommand)]
        action: LoginAction,
    },
    /// Interface web distante : état, ouverture, adresse d'écoute
    Remote {
        #[command(subcommand)]
        action: Option<RemoteAction>,
    },
    /// Plugins d'hébergeurs : installer (permissions montrées d'abord), lister, retirer
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
    /// Jetons de l'interface distante (lecture, ajout ou contrôle ; jamais administration)
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },
    /// Plages horaires d'une file et action de fin (veille, extinction)
    Schedule {
        #[command(subcommand)]
        action: ScheduleAction,
    },
    /// Moteur : démarrage, état, arrêt
    Engine {
        #[command(subcommand)]
        action: EngineAction,
    },
    /// Interroge un serveur : taille, plages acceptées, validateurs et nom du fichier
    Probe {
        /// Adresse HTTP ou HTTPS du fichier
        url: String,
    },
    /// Gestion d'une base de données du moteur
    Db {
        #[command(subcommand)]
        action: DbAction,
    },
    /// Sans moteur : exécute une tâche avec la bibliothèque (bancs, dépannage)
    Direct {
        /// Base de données utilisée directement (jamais celle d'un moteur en marche)
        #[arg(long, default_value = "auro.db")]
        db: PathBuf,
        #[command(subcommand)]
        action: DirectAction,
    },
}

/// Options d'ajout d'une vidéo, communes à `video add` et `video get`.
#[derive(clap::Args)]
struct VideoArgs {
    /// Dossier de destination
    #[arg(short = 'o', long, default_value = ".")]
    dir: PathBuf,
    /// Dossier des téléchargements du moteur au lieu de -o : les règles de rangement
    /// y choisissent le dossier et le nom (auro rules)
    #[arg(long, conflicts_with = "dir")]
    auto: bool,
    /// Format à télécharger (identifiants de auro video info, ou sélecteur de yt-dlp :
    /// « 137+140 », « bv*[height<=720]+ba/b ») ; par défaut la meilleure image et le
    /// meilleur son
    #[arg(short = 'f', long)]
    format: Option<String>,
    /// Conteneur du fichier final (automatique sinon)
    #[arg(long, value_parser = ["mp4", "mkv", "webm"])]
    container: Option<String>,
    /// Langues des sous-titres, gardés à côté de la vidéo : « fr,en »
    #[arg(long = "subs", value_name = "LANGUES", value_delimiter = ',')]
    subtitles: Vec<String>,
    /// File (nom ou identifiant)
    #[arg(short = 'q', long)]
    queue: Option<String>,
    /// Limite de débit de la tâche (500K, 2M…)
    #[arg(short = 'l', long, value_name = "DÉBIT", value_parser = display::parse_limit)]
    limit: Option<Limit>,
}

impl VideoArgs {
    fn options(&self, paused: bool) -> remote::VideoOptions<'_> {
        remote::VideoOptions {
            dir: (!self.auto).then_some(self.dir.as_path()),
            format: self.format.as_deref(),
            container: self.container.as_deref(),
            subtitles: &self.subtitles,
            queue: self.queue.as_deref(),
            limit: self.limit.and_then(|l| l.0),
            paused,
        }
    }
}

#[derive(Subcommand)]
enum VideoAction {
    /// État de yt-dlp et ffmpeg ; avec --install, les télécharge depuis leurs
    /// publications officielles (signature et empreintes vérifiées)
    Tools {
        #[arg(long)]
        install: bool,
        /// Réinstaller même ce qui est déjà là (avec --install)
        #[arg(long, requires = "install")]
        force: bool,
    },
    /// Décrit une page de vidéo : titre, durée, sous-titres et formats proposés
    Info { page: String },
    /// Ajoute des vidéos au moteur, sans attendre
    Add {
        /// Adresses des pages
        #[arg(required = true)]
        pages: Vec<String>,
        #[command(flatten)]
        args: VideoArgs,
        /// Ajouter en pause
        #[arg(long)]
        paused: bool,
    },
    /// Télécharge une vidéo et suit l'avancement (Ctrl-C la met en pause)
    Get {
        page: String,
        #[command(flatten)]
        args: VideoArgs,
    },
}

/// Rangs de fichiers d'un torrent ou d'un Metalink.
#[derive(Clone)]
struct Selection(Vec<u32>);

/// « 0,2-4 » : rangs et intervalles, sans doublon.
/// Une taille : « 800000 », « 500K », « 2M », « 1.5G » (unités binaires).
fn parse_size(text: &str) -> Result<u64, String> {
    match display::parse_limit(text) {
        Ok(display::Limit(Some(bytes))) => Ok(bytes),
        _ => Err(format!(
            "taille illisible : {text} (exemples : 500K, 2M, 1G)"
        )),
    }
}

fn parse_selection(text: &str) -> Result<Selection, String> {
    let rank = |t: &str| {
        t.trim()
            .parse::<u32>()
            .map_err(|_| format!("rang invalide : {t}"))
    };
    let mut ranks = Vec::new();
    for part in text.split(',') {
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (rank(a)?, rank(b)?);
                if a > b {
                    return Err(format!("intervalle à l'envers : {part}"));
                }
                if b - a >= 100_000 {
                    return Err(format!("intervalle trop grand : {part}"));
                }
                ranks.extend(a..=b);
            }
            None => ranks.push(rank(part)?),
        }
    }
    ranks.sort_unstable();
    ranks.dedup();
    Ok(Selection(ranks))
}

#[derive(Subcommand)]
enum QueueAction {
    /// Liste les files
    List,
    /// Crée une file
    Create {
        name: String,
        /// Tâches simultanées (3 par défaut)
        #[arg(long)]
        max_active: Option<u32>,
        /// Limite de débit de toute la file (500K, 2M…)
        #[arg(short = 'l', long, value_name = "DÉBIT", value_parser = display::parse_limit)]
        limit: Option<Limit>,
    },
    /// Modifie une file (nom ou identifiant)
    Update {
        queue: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        max_active: Option<u32>,
        /// Limite de débit de toute la file (500K, 2M…, ou « none »)
        #[arg(short = 'l', long, value_name = "DÉBIT", value_parser = display::parse_limit)]
        limit: Option<Limit>,
    },
}

#[derive(Subcommand)]
enum LoginAction {
    /// Liste les sites et leurs noms d'utilisateur (jamais les mots de passe)
    List,
    /// Enregistre ou modifie les identifiants d'un site. Le mot de passe est demandé sans
    /// écho ; il n'est jamais un argument, qui resterait dans l'historique du terminal
    Set {
        /// Site : exemple.org (ses sous-domaines compris), ou https://exemple.org/dossier
        site: String,
        #[arg(short = 'u', long)]
        username: String,
        /// Lire le mot de passe sur l'entrée standard (une ligne), pour un script
        #[arg(long, conflicts_with = "keep_password")]
        password_stdin: bool,
        /// Garder le mot de passe déjà enregistré (changer seulement le nom d'utilisateur)
        #[arg(long)]
        keep_password: bool,
    },
    /// Retire les identifiants d'un site
    Remove { site: String },
}

#[derive(Subcommand)]
enum ProxyAction {
    /// Réglages du proxy (par défaut) ; jamais le mot de passe
    Show,
    /// Le proxy du système (par défaut) : réglages Internet sous Windows, variables
    /// d'environnement ailleurs
    System,
    /// Aucun proxy : toujours en direct
    None,
    /// Un proxy choisi : http://hôte:port, https://…, socks5://… ou socks5h://… (le proxy
    /// résout alors les noms). Les torrents n'y passent jamais. Avec --user, le mot de passe
    /// est demandé sans écho et rangé au coffre du système
    Manual {
        url: String,
        /// Hôtes joints en direct, séparés par des virgules (localhost,.lan,192.168.0.0/16)
        #[arg(long, value_delimiter = ',', value_name = "HÔTES")]
        except: Vec<String>,
        #[arg(short = 'u', long)]
        user: Option<String>,
        /// Lire le mot de passe sur l'entrée standard (une ligne), pour un script
        #[arg(long, requires = "user", conflicts_with = "keep_password")]
        password_stdin: bool,
        /// Garder le mot de passe déjà enregistré
        #[arg(long, requires = "user")]
        keep_password: bool,
    },
}

#[derive(Subcommand)]
enum RulesAction {
    /// Les règles, dans leur ordre (par défaut)
    Show,
    /// Ajoute une règle (à la fin, ou au rang --at). Critères facultatifs, tous requis ;
    /// sans critère, la règle prend toute tâche
    Add(Box<RuleArgs>),
    /// Retire une règle (rang, identifiant ou nom)
    Remove { rule: String },
    /// Active une règle
    Enable { rule: String },
    /// Désactive une règle
    Disable { rule: String },
    /// Change le rang d'une règle (1 : la première)
    Move { rule: String, rank: usize },
}

/// Une règle à ajouter (`auro rules add`).
#[derive(clap::Args)]
struct RuleArgs {
    /// Nom de la règle
    #[arg(long)]
    name: Option<String>,
    /// Types de fichier : compressed, documents, images, music, programs, video,
    /// other, ou extensions (iso,pdf), séparés par des virgules
    #[arg(long = "type", value_delimiter = ',', value_name = "TYPES")]
    types: Vec<String>,
    /// Domaines du site (sous-domaines compris), séparés par des virgules
    #[arg(long = "domain", value_delimiter = ',', value_name = "DOMAINES")]
    domains: Vec<String>,
    /// Expression régulière sur l'adresse ((?i) pour ignorer la casse)
    #[arg(long, value_name = "EXPRESSION")]
    url_regex: Option<String>,
    /// Expression régulière sur le nom du fichier
    #[arg(long, value_name = "EXPRESSION")]
    name_regex: Option<String>,
    /// Dossier : absolu, ou relatif au dossier des téléchargements (Vidéos/Séries)
    #[arg(short = 'o', long, value_name = "DOSSIER")]
    folder: Option<String>,
    /// Modèle de nom : {nom}, {ext}, {hôte}, {date} (ou {name}, {host}) ; pas pour un
    /// torrent
    #[arg(long, value_name = "MODÈLE")]
    rename: Option<String>,
    /// Commande de fin : un programme en chemin absolu, lancé sans shell quand la tâche
    /// est terminée (Windows et Linux)
    #[arg(long, value_name = "PROGRAMME")]
    exec: Option<String>,
    /// Argument de la commande, à répéter : {chemin}, {dossier}, {nom}, {taille},
    /// {sha256}, {id}
    #[arg(
        long = "arg",
        value_name = "ARGUMENT",
        requires = "exec",
        allow_hyphen_values = true
    )]
    args: Vec<String>,
    /// Délai de la commande en secondes (600 par défaut) : au-delà, la tâche échoue
    #[arg(long, value_name = "SECONDES", requires = "exec")]
    timeout: Option<u32>,
    /// Rang de la nouvelle règle (1 : la première)
    #[arg(long)]
    at: Option<usize>,
    /// Ajoutée désactivée
    #[arg(long)]
    disabled: bool,
}

#[derive(Subcommand)]
enum RemoteAction {
    /// État : réglages et écoute (par défaut)
    Status,
    /// Ouvre l'interface distante (127.0.0.1:7380 par défaut ; hors de la boucle locale, en
    /// TLS, avec un certificat fabriqué par le moteur ou fourni)
    Enable {
        /// Adresse et port d'écoute : 127.0.0.1:7380 (cette machine seulement),
        /// 0.0.0.0:7380 (toutes ses adresses, en TLS), 192.168.1.20:7380…
        #[arg(long)]
        listen: Option<String>,
    },
    /// Noms ajoutés, acceptés et portés par le certificat fabriqué (nom DNS ou adresse IP)
    Hosts {
        /// Les noms (remplacent les précédents)
        #[arg(required_unless_present = "clear")]
        names: Vec<String>,
        /// Aucun nom ajouté
        #[arg(long, conflicts_with = "names")]
        clear: bool,
    },
    /// Certificat de l'écoute en TLS : fourni (fichiers PEM), ou celui fabriqué par le
    /// moteur
    Cert {
        /// Certificat (chaîne PEM)
        #[arg(long, requires = "key", conflicts_with = "generated")]
        file: Option<String>,
        /// Sa clé privée (PEM), lue par le moteur à chaque ouverture, jamais copiée
        #[arg(long, requires = "file")]
        key: Option<String>,
        /// Revenir au certificat fabriqué par le moteur
        #[arg(long, required_unless_present = "file")]
        generated: bool,
    },
    /// Ferme l'interface distante ; ses connexions se ferment
    Disable,
    /// Réseau interne pour les tâches des jetons (adresses privées, boucle locale) : on ou off
    AllowPrivate {
        #[arg(value_parser = ["on", "off"])]
        value: String,
    },
}

#[derive(Subcommand)]
enum PluginAction {
    /// Montre un plugin (fichier .wasm) : pages, domaines demandés, empreinte ; l'installe
    /// avec --yes
    Install {
        file: PathBuf,
        /// Accepter ces permissions et installer
        #[arg(long)]
        yes: bool,
    },
    /// Liste les plugins installés et leurs permissions
    List,
    /// Retire un plugin, avec son fichier
    Remove { plugin: String },
    /// Active un plugin
    Enable { plugin: String },
    /// Désactive un plugin (gardé, mais plus utilisé)
    Disable { plugin: String },
    /// Montre les liens qu'un plugin trouve sur une page, numérotés
    Resolve { page: String },
    /// Ajoute des liens d'une page par son plugin (tous, ou ceux de --select)
    Add {
        page: String,
        /// Dossier de destination
        #[arg(short = 'o', long, default_value = ".")]
        dir: PathBuf,
        /// Liens à ajouter : « 0,2-4 » (rangs montrés par auro plugin resolve)
        #[arg(long, value_name = "RANGS", value_parser = parse_selection)]
        select: Option<Selection>,
        /// Ajouter en pause
        #[arg(long)]
        paused: bool,
    },
}

#[derive(Subcommand)]
enum TokenAction {
    /// Crée un jeton et l'affiche, une seule fois
    Create {
        /// Nom du jeton (l'appareil qui s'en servira)
        name: String,
        /// read : voir ; add : ajouter seulement ; control : tout piloter, sauf les réglages
        #[arg(long, default_value = "control", value_parser = ["read", "add", "control"])]
        scope: String,
    },
    /// Liste les jetons (jamais leur valeur)
    List,
    /// Révoque un jeton (identifiant ou nom) : ses sessions et ses connexions se ferment
    Revoke { token: String },
}

#[derive(Subcommand)]
enum ScheduleAction {
    /// Liste les planifications : plages, ouverture, action de fin, files
    List,
    /// Donne à une file une plage horaire, qui remplace sa planification
    Set {
        /// Jours : « 1-5 », « 6,7 » ou « tous » (1 : lundi, 7 : dimanche)
        #[arg(long, value_parser = parse_days)]
        days: Days,
        /// Début de la plage, HH:MM
        #[arg(long, value_name = "HH:MM")]
        from: String,
        /// Fin de la plage, HH:MM ; avant le début, la plage passe minuit
        #[arg(long, value_name = "HH:MM")]
        to: String,
        /// File (nom ou identifiant ; la principale par défaut)
        #[arg(short = 'q', long)]
        queue: Option<String>,
        /// Quand la file n'a plus rien à télécharger : none, sleep (veille) ou
        /// shutdown (extinction), annoncée 60 s avant
        #[arg(long, default_value = "none", value_parser = ["none", "sleep", "shutdown"])]
        then: String,
    },
    /// Retire la planification d'une file : elle télécharge à toute heure
    Clear {
        /// File (nom ou identifiant ; la principale par défaut)
        #[arg(short = 'q', long)]
        queue: Option<String>,
    },
    /// Annule l'action de fin annoncée (veille, extinction)
    Cancel,
}

/// Jours de la semaine d'une plage, de 1 (lundi) à 7 (dimanche).
#[derive(Clone)]
struct Days(Vec<u8>);

fn parse_days(text: &str) -> Result<Days, String> {
    if matches!(text, "tous" | "all") {
        return Ok(Days((1..=7).collect()));
    }
    let day = |t: &str| {
        t.trim()
            .parse::<u8>()
            .ok()
            .filter(|d| (1..=7).contains(d))
            .ok_or_else(|| format!("jour invalide : {t} (1 : lundi … 7 : dimanche)"))
    };
    let mut days = Vec::new();
    for part in text.split(',') {
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (day(a)?, day(b)?);
                if a > b {
                    return Err(format!("intervalle à l'envers : {part}"));
                }
                days.extend(a..=b);
            }
            None => days.push(day(part)?),
        }
    }
    days.sort_unstable();
    days.dedup();
    Ok(Days(days))
}

#[derive(Subcommand)]
enum EngineAction {
    /// Démarre le moteur s'il ne tourne pas
    Start {
        /// Base du moteur (par défaut, dans le dossier de données de l'utilisateur)
        #[arg(long, value_name = "FICHIER")]
        db: Option<PathBuf>,
    },
    /// Indique si le moteur tourne, et l'état de ses tâches
    Status,
    /// Arrête le moteur ; les tâches en cours repartiront au prochain démarrage
    Stop,
}

#[derive(Subcommand)]
enum DbAction {
    /// Crée la base ou la met à jour vers le dernier schéma
    Migrate {
        /// Chemin du fichier de base de données
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum DirectAction {
    /// Télécharge un fichier (Ctrl-C met la tâche en pause)
    Get {
        url: String,
        #[arg(short = 'o', long, default_value = ".")]
        dir: PathBuf,
        #[arg(short = 'c', long, value_parser = connections())]
        connections: Option<u32>,
    },
    /// Reprend une tâche en pause, en échec ou interrompue
    Resume {
        task_id: String,
        #[arg(short = 'c', long, value_parser = connections())]
        connections: Option<u32>,
    },
    /// Liste les tâches de la base
    List,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let target = Target {
        endpoint: cli.endpoint.clone().unwrap_or_else(paths::endpoint),
        autostart: !cli.no_start,
        ca_cert: cli.ca_cert.clone(),
    };
    match cli.command {
        Command::Probe { url } => probe(&url, cli.ca_cert.as_deref()),
        Command::Db {
            action: DbAction::Migrate { path },
        } => {
            let conn = direct::open_db(&path)?;
            let version = auro_engine::store::current_version(&conn)?;
            println!("Base prête : {} (schéma version {version})", path.display());
            Ok(())
        }
        Command::Direct { db, action } => {
            let client = http_client(cli.ca_cert.as_deref())?;
            match action {
                DirectAction::Get {
                    url,
                    dir,
                    connections,
                } => direct::get(&db, &url, &dir, connections, client),
                DirectAction::Resume {
                    task_id,
                    connections,
                } => direct::resume(&db, &task_id, connections, client),
                DirectAction::List => direct::list(&db),
            }
        }
        command => runtime()?.block_on(engine_command(command, &target)),
    }
}

async fn engine_command(command: Command, target: &Target) -> Result<()> {
    match command {
        Command::Get {
            url,
            dir,
            auto,
            connections,
            queue,
            limit,
            headers,
            checksum,
            mirrors,
            select,
        } => {
            let opts = AddOptions {
                dir: (!auto).then_some(dir.as_path()),
                connections,
                queue: queue.as_deref(),
                limit: limit.and_then(|l| l.0),
                paused: false,
                headers: &headers,
                checksum: checksum.as_ref(),
                mirrors: &mirrors,
                select: select.as_ref().map(|s| s.0.as_slice()),
                no_plugin: false,
            };
            remote::get(target, remote::NewTask::File(&url, &opts)).await
        }
        Command::Video {
            action: VideoAction::Get { page, args },
        } => {
            let opts = args.options(false);
            remote::get(target, remote::NewTask::Video(&page, &opts)).await
        }
        Command::Engine { action } => engine(action, target).await,
        command => {
            let mut rpc = client::connect(target).await?;
            match command {
                Command::Add {
                    urls,
                    dir,
                    auto,
                    connections,
                    queue,
                    limit,
                    paused,
                    headers,
                    checksum,
                    mirrors,
                    select,
                    no_plugin,
                } => {
                    if checksum.is_some() && urls.len() > 1 {
                        bail!("--checksum : une seule adresse, l'empreinte est celle d'un fichier");
                    }
                    if !mirrors.is_empty() && urls.len() > 1 {
                        bail!("--mirror : une seule adresse, les miroirs sont ceux d'un fichier");
                    }
                    let opts = AddOptions {
                        dir: (!auto).then_some(dir.as_path()),
                        connections,
                        queue: queue.as_deref(),
                        limit: limit.and_then(|l| l.0),
                        paused,
                        headers: &headers,
                        checksum: checksum.as_ref(),
                        mirrors: &mirrors,
                        select: select.as_ref().map(|s| s.0.as_slice()),
                        no_plugin,
                    };
                    remote::add(&mut rpc, &urls, &opts).await
                }
                Command::List { states, queue } => {
                    remote::list(&mut rpc, &states, queue.as_deref()).await
                }
                Command::Pause { task_ids } => {
                    remote::act(&mut rpc, "task.pause", &task_ids, json!({})).await
                }
                Command::Resume { task_ids } => {
                    remote::act(&mut rpc, "task.resume", &task_ids, json!({})).await
                }
                Command::Cancel {
                    task_ids,
                    delete_files,
                } => {
                    let extra = json!({"deleteFiles": delete_files});
                    remote::act(&mut rpc, "task.cancel", &task_ids, extra).await
                }
                Command::Remove {
                    task_ids,
                    delete_files,
                } => {
                    let extra = json!({"deleteFiles": delete_files});
                    remote::act(&mut rpc, "task.remove", &task_ids, extra).await
                }
                Command::Restart {
                    task_ids,
                    delete_files,
                } => {
                    let extra = json!({"deleteFiles": delete_files});
                    remote::act(&mut rpc, "task.restart", &task_ids, extra).await
                }
                Command::Move { task_id, dir, name } => {
                    remote::relocate(&mut rpc, &task_id, dir.as_deref(), name.as_deref()).await
                }
                Command::Refresh { task_id, url } => {
                    remote::refresh(&mut rpc, &task_id, &url).await
                }
                Command::Mirror { task_id, url } => match url {
                    Some(url) => remote::add_mirror(&mut rpc, &task_id, &url).await,
                    None => remote::sources(&mut rpc, &task_id).await,
                },
                // `--clear` exclut une empreinte : sans elle, c'est un retrait.
                Command::Checksum {
                    task_id, checksum, ..
                } => remote::set_checksum(&mut rpc, &task_id, checksum.as_ref()).await,
                Command::Watch { task_ids } => remote::watch(&mut rpc, &task_ids).await,
                Command::Metalink { source } => remote::metalink(&mut rpc, &source).await,
                Command::Links {
                    source,
                    extensions,
                    regex,
                    min_size,
                    max_size,
                    all,
                    no_sizes,
                    add,
                    dir,
                    paused,
                } => {
                    let regex = regex
                        .map(|r| regex::Regex::new(&r))
                        .transpose()
                        .context("--regex : expression illisible")?;
                    let filters = links::Filters {
                        extensions: extensions
                            .iter()
                            .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
                            .filter(|e| !e.is_empty())
                            .collect(),
                        regex,
                        min_size,
                        max_size,
                        all,
                    };
                    let opts = links::LinksOptions {
                        source: &source,
                        filters,
                        no_sizes,
                        add: add.then_some(dir.as_path()),
                        paused,
                    };
                    links::links(&mut rpc, opts).await
                }
                Command::Files { task_id, select } => {
                    let select = select.as_ref().map(|s| s.0.as_slice());
                    remote::files(&mut rpc, &task_id, select).await
                }
                Command::Limit {
                    rate,
                    task,
                    queue,
                    upload,
                } => {
                    let to = match (task.as_deref(), queue.as_deref(), upload) {
                        (_, _, true) => remote::LimitOf::Upload,
                        (Some(t), _, _) => remote::LimitOf::Task(t),
                        (None, Some(q), _) => remote::LimitOf::Queue(q),
                        (None, None, false) => remote::LimitOf::Global,
                    };
                    remote::limit(&mut rpc, to, rate).await
                }
                Command::Config { key, value } => {
                    remote::config(&mut rpc, key.as_deref(), value.as_deref()).await
                }
                Command::Video { action } => match action {
                    VideoAction::Tools { install, force } => {
                        video::tools(&mut rpc, install, force).await
                    }
                    VideoAction::Info { page } => video::info(&mut rpc, &page).await,
                    VideoAction::Add {
                        pages,
                        args,
                        paused,
                    } => remote::video_add(&mut rpc, &pages, &args.options(paused)).await,
                    VideoAction::Get { .. } => unreachable!("suivie à part"),
                },
                Command::History { query, csv, clear } => match clear {
                    true => remote::clear_history(&mut rpc).await,
                    false => remote::history(&mut rpc, query.as_deref(), csv).await,
                },
                Command::Diagnostic { save } => remote::diagnostic(&mut rpc, save.as_deref()).await,
                Command::Rules { action } => match action.unwrap_or(RulesAction::Show) {
                    RulesAction::Show => remote::rules_show(&mut rpc).await,
                    RulesAction::Add(rule) => {
                        let RuleArgs {
                            name,
                            types,
                            domains,
                            url_regex,
                            name_regex,
                            folder,
                            rename,
                            exec,
                            args,
                            timeout,
                            at,
                            disabled,
                        } = *rule;
                        let (categories, extensions): (Vec<String>, Vec<String>) = types
                            .into_iter()
                            .partition(|t| remote::RULE_CATEGORIES.contains(&t.as_str()));
                        let command = exec.map(|program| {
                            let mut c = json!({"program": program, "args": args});
                            if let Some(t) = timeout {
                                c["timeoutS"] = json!(t);
                            }
                            c
                        });
                        let rule = json!({
                            "name": name,
                            "enabled": !disabled,
                            "match": {
                                "categories": categories,
                                "extensions": extensions,
                                "domains": domains,
                                "urlRegex": url_regex,
                                "nameRegex": name_regex,
                            },
                            "actions": {"folder": folder, "rename": rename, "command": command},
                        });
                        remote::rules_add(&mut rpc, rule, at).await
                    }
                    RulesAction::Remove { rule } => {
                        remote::rules_edit(&mut rpc, &rule, remote::RuleEdit::Remove).await
                    }
                    RulesAction::Enable { rule } => {
                        remote::rules_edit(&mut rpc, &rule, remote::RuleEdit::Enable(true)).await
                    }
                    RulesAction::Disable { rule } => {
                        remote::rules_edit(&mut rpc, &rule, remote::RuleEdit::Enable(false)).await
                    }
                    RulesAction::Move { rule, rank } => {
                        remote::rules_edit(&mut rpc, &rule, remote::RuleEdit::MoveTo(rank)).await
                    }
                },
                Command::Proxy { action } => match action.unwrap_or(ProxyAction::Show) {
                    ProxyAction::Show => remote::proxy_show(&mut rpc).await,
                    ProxyAction::System => {
                        remote::proxy_set(&mut rpc, json!({"mode": "system"})).await
                    }
                    ProxyAction::None => remote::proxy_set(&mut rpc, json!({"mode": "none"})).await,
                    ProxyAction::Manual {
                        url,
                        except,
                        user,
                        password_stdin,
                        keep_password,
                    } => {
                        let mut params =
                            json!({"mode": "manual", "url": url, "exceptions": except});
                        if let Some(user) = user {
                            params["username"] = json!(user);
                            if !keep_password {
                                params["password"] = json!(match password_stdin {
                                    true => display::read_line_stdin()?,
                                    false => rpassword::prompt_password("Mot de passe du proxy : ")
                                        .context("lecture du mot de passe")?,
                                });
                            }
                        }
                        remote::proxy_set(&mut rpc, params).await
                    }
                },
                Command::Dir { path, reset } => {
                    remote::download_dir(&mut rpc, path.as_deref(), reset).await
                }
                Command::Login { action } => match action {
                    LoginAction::List => remote::logins(&mut rpc).await,
                    LoginAction::Set {
                        site,
                        username,
                        password_stdin,
                        keep_password,
                    } => {
                        let password = match (keep_password, password_stdin) {
                            (true, _) => None,
                            (false, true) => Some(display::read_line_stdin()?),
                            (false, false) => Some(
                                rpassword::prompt_password("Mot de passe : ")
                                    .context("lecture du mot de passe")?,
                            ),
                        };
                        remote::login_set(&mut rpc, &site, &username, password).await
                    }
                    LoginAction::Remove { site } => remote::login_remove(&mut rpc, &site).await,
                },
                Command::Remote { action } => match action.unwrap_or(RemoteAction::Status) {
                    RemoteAction::Status => remote::remote_status(&mut rpc).await,
                    RemoteAction::Enable { listen } => {
                        let change = remote::RemoteChange {
                            enabled: Some(true),
                            listen: listen.as_deref(),
                            ..Default::default()
                        };
                        remote::remote_set(&mut rpc, change).await
                    }
                    RemoteAction::Disable => {
                        let change = remote::RemoteChange {
                            enabled: Some(false),
                            ..Default::default()
                        };
                        remote::remote_set(&mut rpc, change).await
                    }
                    RemoteAction::AllowPrivate { value } => {
                        let change = remote::RemoteChange {
                            allow_private: Some(value == "on"),
                            ..Default::default()
                        };
                        remote::remote_set(&mut rpc, change).await
                    }
                    RemoteAction::Hosts { names, clear } => {
                        let change = remote::RemoteChange {
                            hosts: Some(if clear { Vec::new() } else { names }),
                            ..Default::default()
                        };
                        remote::remote_set(&mut rpc, change).await
                    }
                    RemoteAction::Cert { file, key, .. } => {
                        let change = remote::RemoteChange {
                            tls_files: Some(file.zip(key)),
                            ..Default::default()
                        };
                        remote::remote_set(&mut rpc, change).await
                    }
                },
                Command::Plugin { action } => match action {
                    PluginAction::Install { file, yes } => {
                        remote::plugin_install(&mut rpc, &file, yes).await
                    }
                    PluginAction::List => remote::plugins(&mut rpc).await,
                    PluginAction::Remove { plugin } => {
                        remote::plugin_remove(&mut rpc, &plugin).await
                    }
                    PluginAction::Enable { plugin } => {
                        remote::plugin_enable(&mut rpc, &plugin, true).await
                    }
                    PluginAction::Disable { plugin } => {
                        remote::plugin_enable(&mut rpc, &plugin, false).await
                    }
                    PluginAction::Resolve { page } => remote::plugin_resolve(&mut rpc, &page).await,
                    PluginAction::Add {
                        page,
                        dir,
                        select,
                        paused,
                    } => {
                        let select = select.as_ref().map(|s| s.0.as_slice());
                        remote::plugin_add(&mut rpc, &page, select, &dir, paused).await
                    }
                },
                Command::Token { action } => match action {
                    TokenAction::Create { name, scope } => {
                        remote::token_create(&mut rpc, &name, &scope).await
                    }
                    TokenAction::List => remote::tokens(&mut rpc).await,
                    TokenAction::Revoke { token } => remote::token_revoke(&mut rpc, &token).await,
                },
                Command::Schedule { action } => match action {
                    ScheduleAction::List => remote::schedules(&mut rpc).await,
                    ScheduleAction::Set {
                        days,
                        from,
                        to,
                        queue,
                        then,
                    } => {
                        let spec = remote::WindowSpec {
                            days: &days.0,
                            from: &from,
                            to: &to,
                            then: &then,
                        };
                        remote::schedule_set(&mut rpc, queue.as_deref(), &spec).await
                    }
                    ScheduleAction::Clear { queue } => {
                        remote::schedule_clear(&mut rpc, queue.as_deref()).await
                    }
                    ScheduleAction::Cancel => remote::cancel_end_action(&mut rpc).await,
                },
                Command::Queue { action } => match action {
                    QueueAction::List => remote::queues(&mut rpc).await,
                    QueueAction::Create {
                        name,
                        max_active,
                        limit,
                    } => {
                        let limit = limit.and_then(|l| l.0);
                        remote::queue_create(&mut rpc, &name, max_active, limit).await
                    }
                    QueueAction::Update {
                        queue,
                        name,
                        max_active,
                        limit,
                    } => {
                        let change = remote::QueueEdit {
                            name: name.as_deref(),
                            max_active,
                            limit,
                        };
                        remote::queue_update(&mut rpc, &queue, &change).await
                    }
                },
                _ => unreachable!("commandes sans moteur traitées dans main"),
            }
        }
    }
}

async fn engine(action: EngineAction, target: &Target) -> Result<()> {
    let endpoint = &target.endpoint;
    match action {
        EngineAction::Start { db } => {
            if Rpc::connect(endpoint).await.is_ok() {
                println!("Le moteur tourne déjà sur {endpoint}.");
                return Ok(());
            }
            client::start_daemon(endpoint, db.as_deref(), target.ca_cert.as_deref())?;
            client::wait_for(endpoint).await?;
            println!("Moteur démarré sur {endpoint}.");
            Ok(())
        }
        EngineAction::Status => match Rpc::connect(endpoint).await {
            Ok(mut rpc) => remote::status(&mut rpc, endpoint).await,
            Err(_) => {
                println!("Aucun moteur sur {endpoint}.");
                Ok(())
            }
        },
        EngineAction::Stop => {
            let Ok(mut rpc) = Rpc::connect(endpoint).await else {
                println!("Aucun moteur sur {endpoint}.");
                return Ok(());
            };
            rpc.call("engine.shutdown", json!({"graceful": true}))
                .await?;
            // Le moteur ferme la connexion quand il a tout enregistré.
            let _ = rpc.notification().await;
            println!("Moteur arrêté.");
            Ok(())
        }
    }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
}

fn http_client(ca: Option<&Path>) -> Result<Client> {
    Ok(match ca {
        None => Client::new()?,
        Some(path) => {
            let pem = std::fs::read(path)
                .with_context(|| format!("impossible de lire {}", path.display()))?;
            Client::with_extra_roots(&pem)?
        }
    })
}

fn probe(url: &str, ca: Option<&Path>) -> Result<()> {
    let runtime = runtime()?;
    let client = http_client(ca)?;
    let probe = match runtime.block_on(client.probe(url, &[])) {
        Ok(probe) => probe,
        Err(error) => {
            let code = error.error_code().unwrap_or("demande invalide");
            let retry = if error.retryable() {
                "nouvelle tentative possible"
            } else {
                "échec définitif"
            };
            bail!("{error} [{code}, {retry}]");
        }
    };

    // Sans la requête : elle peut porter un jeton (SPEC §8.2).
    let mut shown = probe.final_url.clone();
    shown.set_query(None);
    println!("Adresse finale  : {shown}");
    println!("Nom du fichier  : {}", probe.file_name);
    match probe.total_bytes {
        Some(total) => println!("Taille          : {total} octets"),
        None => println!("Taille          : inconnue"),
    }
    if probe.accepts_ranges {
        println!("Plages          : acceptées (segmentation et reprise possibles)");
    } else {
        println!("Plages          : refusées (un seul flux, reprise impossible)");
    }
    println!(
        "ETag            : {}",
        probe.etag.as_deref().unwrap_or("absent")
    );
    println!(
        "Last-Modified   : {}",
        probe.last_modified.as_deref().unwrap_or("absent")
    );
    if probe.downgraded {
        println!("Attention       : une redirection est passée de HTTPS à HTTP");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_are_read_as_lists_ranges_or_all() {
        assert_eq!(parse_days("1-5").unwrap().0, [1, 2, 3, 4, 5]);
        assert_eq!(parse_days("7,6,6").unwrap().0, [6, 7]);
        assert_eq!(parse_days("1,3-4").unwrap().0, [1, 3, 4]);
        assert_eq!(parse_days("tous").unwrap().0, [1, 2, 3, 4, 5, 6, 7]);
        for bad in ["0", "8", "5-1", "lun", ""] {
            assert!(parse_days(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn file_ranks_are_read_as_lists_and_ranges() {
        assert_eq!(parse_selection("0").unwrap().0, [0]);
        assert_eq!(parse_selection("4,0,2-3,3").unwrap().0, [0, 2, 3, 4]);
        for bad in ["3-1", "a", "", "-1", "0-200000"] {
            assert!(parse_selection(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn setting_values_are_typed() {
        use serde_json::{Value, json};
        assert_eq!(remote::setting_value("oui"), json!(true));
        assert_eq!(remote::setting_value("OFF"), json!(false));
        assert_eq!(remote::setting_value("none"), Value::Null);
        assert_eq!(remote::setting_value("2"), json!(2));
        assert_eq!(remote::setting_value("1.5"), json!(1.5));
        assert_eq!(remote::setting_value("Ethernet 2"), json!("Ethernet 2"));
        assert_eq!(remote::setting_value("inf"), json!("inf"));
    }
}
