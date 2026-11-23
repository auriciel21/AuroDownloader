//! Règles de rangement et commande de fin (F-32, F-34, SPEC §7.21).
//!
//! Une règle range le fichier d'une tâche (dossier, nom) à sa préparation, dès que le nom
//! est connu et avant la création du `.part`, et peut lancer une commande quand la tâche
//! est terminée. La première règle activée qui correspond, dans l'ordre, l'emporte, seule.
//! Les règles ne rangent que les tâches ajoutées sans dossier ; une tâche ajoutée avec un
//! dossier garde sa règle (pour la commande) mais ni son dossier ni son modèle de nom.
//!
//! Ce module ne touche ni à la base ni au disque : la table `rules` est lue et écrite par
//! `store::rules`, le dossier créé et le nom pris par la préparation de chaque sorte de
//! tâche.

use std::path::{Path, PathBuf};

use regex::{Regex, RegexBuilder};
use serde_json::{Map, Value, json};

use crate::net::filename::{NameRules, sanitize};

/// Règles au plus.
pub const MAX_RULES: usize = 100;
/// Valeurs au plus dans chaque liste d'une règle (catégories, extensions, domaines).
pub const MAX_LIST: usize = 50;
/// Longueur au plus d'une expression régulière, en caractères.
pub const MAX_REGEX: usize = 1000;
/// Parties au plus d'un dossier relatif.
pub const MAX_FOLDER_PARTS: usize = 8;
/// Délai d'une commande de fin par défaut, en secondes.
pub const DEFAULT_TIMEOUT_S: u32 = 600;
/// Délai d'une commande de fin au plus, en secondes (un jour).
pub const MAX_TIMEOUT_S: u32 = 86_400;
/// Arguments au plus d'une commande, et longueur au plus de chacun (octets).
pub const MAX_ARGS: usize = 64;
pub const MAX_ARG_BYTES: usize = 4096;
/// Taille au plus de l'automate d'une expression : une expression démesurée est refusée
/// à l'écriture, jamais compilée sans borne.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// Types de fichier, comme ceux de la page (`ui/src/model.js`, `CATEGORIES`), comparés
/// par un test.
pub const CATEGORIES: [&str; 7] = [
    "compressed",
    "documents",
    "images",
    "music",
    "programs",
    "video",
    "other",
];

/// Extensions de chaque type, recopiées de `ui/src/model.js` (`EXTENSIONS`).
const EXTENSIONS: [(&str, &str); 6] = [
    (
        "compressed",
        "7z ace arj bz2 cab gz lz lzma rar tar tbz2 tgz txz xz z zip zst",
    ),
    (
        "documents",
        "chm csv djvu doc docx epub md mobi odp ods odt pdf pps ppt pptx rtf txt xls xlsx",
    ),
    (
        "images",
        "avif bmp gif heic ico jpeg jpg png psd svg tif tiff webp",
    ),
    (
        "music",
        "aac aif aiff alac ape flac m4a mid mp3 mpa oga opus ra wav wma",
    ),
    (
        "programs",
        "apk appimage appx bat deb dmg exe flatpak img iso jar msi msix msu pkg rpm run sh snap",
    ),
    (
        "video",
        "3gp asf avi flv m2ts m4v mkv mov mp4 mpe mpeg mpg ogv qt rm rmvb ts vob webm wmv",
    ),
];

/// Variables du modèle de nom (français et anglais).
const NAME_VARIABLES: [&str; 6] = ["nom", "name", "ext", "hôte", "host", "date"];

/// Variables des arguments d'une commande (français et anglais).
pub const COMMAND_VARIABLES: [&str; 10] = [
    "chemin", "path", "dossier", "dir", "nom", "name", "taille", "size", "sha256", "id",
];

/// Nom et extension d'un nom de fichier : l'extension suit le dernier point, sauf en
/// tête (`.bashrc` n'en a pas).
pub fn split_name(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot + 1..]),
        _ => (name, ""),
    }
}

/// Type d'un fichier d'après son extension, comme la page ; les archives découpées
/// (`.r00`, `.7z.001`) sont des archives.
pub fn category(name: &str) -> &'static str {
    let (_, ext) = split_name(name);
    if ext.is_empty() {
        return "other";
    }
    let ext = ext.to_lowercase();
    let digits = |s: &str| s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit());
    if (ext.len() == 3 && (ext.starts_with('r') || ext.starts_with('z')) && digits(&ext[1..]))
        || (ext.len() == 3 && ext.bytes().all(|b| b.is_ascii_digit()))
    {
        return "compressed";
    }
    for (cat, list) in EXTENSIONS {
        if list.split(' ').any(|e| e == ext) {
            return cat;
        }
    }
    "other"
}

/// Une règle, au format du protocole (`rules.list`, `rules.set`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rule {
    /// Vide pour une règle nouvelle, avant son enregistrement.
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub criteria: Criteria,
    pub actions: Actions,
}

/// Critères : chacun facultatif, tous doivent correspondre.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Criteria {
    pub categories: Vec<String>,
    /// Sans le point, en minuscules.
    pub extensions: Vec<String>,
    /// En minuscules ; un domaine vaut aussi pour ses sous-domaines.
    pub domains: Vec<String>,
    pub url_regex: Option<String>,
    pub name_regex: Option<String>,
}

/// Actions d'une règle.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Actions {
    /// Absolu, ou relatif au dossier des téléchargements (parties séparées par `/`).
    pub folder: Option<String>,
    /// Modèle de nom.
    pub rename: Option<String>,
    pub command: Option<Command>,
}

/// Commande de fin (F-34) : un programme et ses arguments, sans shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub timeout_s: u32,
}

/// Ce qu'une règle compare (SPEC §7.21). Une adresse comparée n'est jamais journalisée.
#[derive(Debug, Clone, Copy)]
pub struct Subject<'a> {
    /// Adresse de la source principale, de la page d'une vidéo, ou lien magnet.
    pub url: &'a str,
    /// Hôte de cette adresse ; `None` pour un torrent.
    pub host: Option<&'a str>,
    /// Nom du fichier avant le modèle (celui choisi par l'utilisateur, sinon celui du
    /// serveur, du titre ou du torrent).
    pub name: &'a str,
}

/// Rangement retenu pour une tâche à sa préparation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Placement {
    /// Règle retenue (sa commande de fin s'exécutera) ; `None` : aucune.
    pub rule_id: Option<String>,
    /// Nouveau dossier (à créer s'il manque) ; `None` : celui de la tâche.
    pub dir: Option<PathBuf>,
    /// Nouveau nom (passé par `sanitize`) ; `None` : celui d'origine.
    pub name: Option<String>,
}

/// Ce qui borne le rangement d'une tâche.
#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    /// Ajoutée sans dossier : le dossier et le nom de la règle s'appliquent.
    pub auto_dest: bool,
    /// Dossier de la tâche à son ajout (base d'un dossier relatif).
    pub base: &'a Path,
    /// Le modèle de nom peut s'appliquer : pas de nom choisi par l'utilisateur, pas un
    /// torrent.
    pub rename: bool,
    /// Jour de l'ajout, heure locale (`AAAA-MM-JJ`).
    pub date: &'a str,
}

/// Rangement d'une tâche : la première règle activée qui correspond.
pub fn place(rules: &[Rule], subject: &Subject<'_>, scope: &Scope<'_>) -> Placement {
    let Some(rule) = rules
        .iter()
        .find(|r| r.enabled && r.criteria.matches(subject))
    else {
        return Placement::default();
    };
    let mut placement = Placement {
        rule_id: Some(rule.id.clone()),
        ..Placement::default()
    };
    if !scope.auto_dest {
        return placement;
    }
    if let Some(folder) = &rule.actions.folder {
        placement.dir = Some(folder_dir(scope.base, folder));
    }
    if scope.rename
        && let Some(template) = &rule.actions.rename
    {
        placement.name = render_name(template, subject.name, subject.host, scope.date)
            .filter(|n| n != subject.name);
    }
    placement
}

impl Criteria {
    /// La règle prend-elle ce fichier ? Une règle sans critère prend tout.
    pub fn matches(&self, subject: &Subject<'_>) -> bool {
        if !self.categories.is_empty() || !self.extensions.is_empty() {
            let (_, ext) = split_name(subject.name);
            let ext = ext.to_lowercase();
            let by_category = self.categories.iter().any(|c| c == category(subject.name));
            let by_extension = !ext.is_empty() && self.extensions.contains(&ext);
            if !by_category && !by_extension {
                return false;
            }
        }
        if !self.domains.is_empty() {
            let Some(host) = subject.host else {
                return false;
            };
            let host = host.trim_end_matches('.').to_lowercase();
            let covered = self.domains.iter().any(|d| {
                host == *d
                    || (host.len() > d.len()
                        && host.ends_with(d.as_str())
                        && host.as_bytes()[host.len() - d.len() - 1] == b'.')
            });
            if !covered {
                return false;
            }
        }
        let fits = |pattern: &Option<String>, text: &str| match pattern {
            None => true,
            Some(p) => compile(p).is_ok_and(|re| re.is_match(text)),
        };
        fits(&self.url_regex, subject.url) && fits(&self.name_regex, subject.name)
    }
}

/// Compile une expression avec un automate borné.
fn compile(pattern: &str) -> Result<Regex, regex::Error> {
    RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .build()
}

/// Dossier d'une règle : absolu tel quel, relatif sous `base`.
pub fn folder_dir(base: &Path, folder: &str) -> PathBuf {
    let path = Path::new(folder);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    folder
        .split('/')
        .fold(base.to_path_buf(), |dir, part| dir.join(part))
}

/// Nom d'un fichier selon un modèle ; `None` si le résultat est vide. Le résultat contient
/// le nom du serveur : il passe par `sanitize`.
pub fn render_name(template: &str, name: &str, host: Option<&str>, date: &str) -> Option<String> {
    let (stem, ext) = split_name(name);
    let template = if ext.is_empty() {
        template.replace(".{ext}", "")
    } else {
        template.to_owned()
    };
    let rendered = substitute(&template, |var| match var {
        "nom" | "name" => Some(stem.to_owned()),
        "ext" => Some(ext.to_owned()),
        "hôte" | "host" => Some(host.unwrap_or_default().to_owned()),
        "date" => Some(date.to_owned()),
        _ => None,
    })?;
    let rendered = rendered.trim();
    if rendered.is_empty() {
        return None;
    }
    Some(sanitize(rendered, NameRules::native()))
}

/// Remplace chaque `{variable}` d'un texte ; `None` si une variable est inconnue ou une
/// accolade seule.
pub fn substitute(text: &str, value: impl Fn(&str) -> Option<String>) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(['{', '}']) {
        if rest[open..].starts_with('}') {
            return None;
        }
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after.find('}')?;
        let var = &after[..close];
        if var.contains('{') {
            return None;
        }
        out.push_str(&value(var)?);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// Les variables d'un texte sont-elles toutes parmi `known` ?
fn check_variables(text: &str, known: &[&str]) -> bool {
    substitute(text, |v| known.contains(&v).then(String::new)).is_some()
}

impl Rule {
    /// Règle lue du protocole, vérifiée. `id` : celui donné, sinon vide.
    pub fn from_json(value: &Value) -> Result<Rule, String> {
        let obj = value.as_object().ok_or("une règle est un objet")?;
        known_keys(
            obj,
            &["ruleId", "name", "enabled", "match", "actions"],
            "règle",
        )?;
        let id = match obj.get("ruleId") {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(s)) if !s.is_empty() && s.len() <= 64 => s.clone(),
            Some(_) => return Err("ruleId : un identifiant".into()),
        };
        let name = match obj.get("name") {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(s)) => s.trim().to_owned(),
            Some(_) => return Err("name : un texte".into()),
        };
        if name.chars().count() > 100 || name.chars().any(char::is_control) {
            return Err("name : 100 caractères au plus, sans caractère de contrôle".into());
        }
        let enabled = match obj.get("enabled") {
            None | Some(Value::Null) => true,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err("enabled : un booléen".into()),
        };
        let criteria = Criteria::from_json(obj.get("match").unwrap_or(&Value::Null))?;
        let actions = Actions::from_json(obj.get("actions").unwrap_or(&Value::Null))?;
        Ok(Rule {
            id,
            name,
            enabled,
            criteria,
            actions,
        })
    }

    /// Règle au format du protocole. `commands` : faux pour une connexion à jeton, qui ne
    /// voit pas les commandes (SPEC §7.21), seulement qu'il y en a une.
    pub fn to_json(&self, commands: bool) -> Value {
        json!({
            "ruleId": self.id,
            "name": self.name,
            "enabled": self.enabled,
            "match": self.criteria.to_json(),
            "actions": self.actions.to_json(commands),
        })
    }
}

impl Criteria {
    pub fn from_json(value: &Value) -> Result<Criteria, String> {
        let empty = Map::new();
        let obj = match value {
            Value::Null => &empty,
            Value::Object(o) => o,
            _ => return Err("match : un objet".into()),
        };
        known_keys(
            obj,
            &[
                "categories",
                "extensions",
                "domains",
                "urlRegex",
                "nameRegex",
            ],
            "match",
        )?;
        let categories = strings(obj.get("categories"), "match.categories")?;
        if let Some(bad) = categories
            .iter()
            .find(|c| !CATEGORIES.contains(&c.as_str()))
        {
            return Err(format!("match.categories : type inconnu {bad:?}"));
        }
        let extensions: Vec<String> = strings(obj.get("extensions"), "match.extensions")?
            .into_iter()
            .map(|e| e.trim_start_matches('.').to_lowercase())
            .collect();
        if let Some(bad) = extensions
            .iter()
            .find(|e| e.is_empty() || e.len() > 16 || !e.bytes().all(|b| b.is_ascii_alphanumeric()))
        {
            return Err(format!(
                "match.extensions : {bad:?} n'est pas une extension (lettres et chiffres, 16 au plus)"
            ));
        }
        let domains: Vec<String> = strings(obj.get("domains"), "match.domains")?
            .into_iter()
            .map(|d| d.trim_end_matches('.').to_lowercase())
            .collect();
        if let Some(bad) = domains.iter().find(|d| !is_domain(d)) {
            return Err(format!(
                "match.domains : {bad:?} n'est pas un nom de domaine (ni adresse IP ni joker)"
            ));
        }
        let url_regex = regex_field(obj.get("urlRegex"), "match.urlRegex")?;
        let name_regex = regex_field(obj.get("nameRegex"), "match.nameRegex")?;
        Ok(Criteria {
            categories,
            extensions,
            domains,
            url_regex,
            name_regex,
        })
    }

    pub fn to_json(&self) -> Value {
        json!({
            "categories": self.categories,
            "extensions": self.extensions,
            "domains": self.domains,
            "urlRegex": self.url_regex,
            "nameRegex": self.name_regex,
        })
    }
}

impl Actions {
    pub fn from_json(value: &Value) -> Result<Actions, String> {
        let empty = Map::new();
        let obj = match value {
            Value::Null => &empty,
            Value::Object(o) => o,
            _ => return Err("actions : un objet".into()),
        };
        known_keys(obj, &["folder", "rename", "command"], "actions")?;
        let folder = optional_text(obj.get("folder"), "actions.folder")?;
        if let Some(folder) = &folder {
            check_folder(folder)?;
        }
        let rename = optional_text(obj.get("rename"), "actions.rename")?;
        if let Some(template) = &rename {
            check_template(template)?;
        }
        let command = match obj.get("command") {
            None | Some(Value::Null) => None,
            Some(value) => Some(Command::from_json(value)?),
        };
        Ok(Actions {
            folder,
            rename,
            command,
        })
    }

    pub fn to_json(&self, commands: bool) -> Value {
        let mut out = json!({
            "folder": self.folder,
            "rename": self.rename,
        });
        if commands {
            out["command"] = self.command.as_ref().map_or(Value::Null, Command::to_json);
        } else {
            out["hasCommand"] = json!(self.command.is_some());
        }
        out
    }
}

impl Command {
    pub fn from_json(value: &Value) -> Result<Command, String> {
        if cfg!(target_os = "android") {
            return Err("actions.command : pas de commande de fin sous Android".into());
        }
        let obj = value.as_object().ok_or("actions.command : un objet")?;
        known_keys(obj, &["program", "args", "timeoutS"], "actions.command")?;
        let program = optional_text(obj.get("program"), "actions.command.program")?
            .ok_or("actions.command.program : le chemin du programme")?;
        if !Path::new(&program).is_absolute() || program.chars().any(char::is_control) {
            return Err("actions.command.program : un chemin absolu".into());
        }
        // Windows lance un `.bat` ou un `.cmd` par `cmd.exe`, un shell qui interpréterait les
        // arguments (nom du fichier venu du serveur compris) : refusé partout, la règle est
        // « jamais de shell » (SPEC §7.21, audit P18, décision 9).
        let extension = Path::new(&program)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        if matches!(extension.as_deref(), Some("bat" | "cmd")) {
            return Err(
                "actions.command.program : un script .bat ou .cmd passe par cmd.exe (un \
                 shell) ; donner un programme, par exemple powershell.exe avec -File"
                    .into(),
            );
        }
        let args = match obj.get("args") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(list)) => list
                .iter()
                .map(|a| a.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or("actions.command.args : des textes")?,
            Some(_) => return Err("actions.command.args : une liste de textes".into()),
        };
        if args.len() > MAX_ARGS {
            return Err(format!("actions.command.args : {MAX_ARGS} au plus"));
        }
        for arg in &args {
            if arg.len() > MAX_ARG_BYTES || arg.contains('\0') {
                return Err(format!(
                    "actions.command.args : {MAX_ARG_BYTES} octets au plus, sans caractère nul"
                ));
            }
            if !check_variables(arg, &COMMAND_VARIABLES) {
                return Err(format!(
                    "actions.command.args : variable inconnue ou accolade seule dans {arg:?} \
                     (variables : {})",
                    COMMAND_VARIABLES.join(", ")
                ));
            }
        }
        let timeout_s = match obj.get("timeoutS") {
            None | Some(Value::Null) => DEFAULT_TIMEOUT_S,
            Some(v) => v
                .as_u64()
                .filter(|t| (1..=u64::from(MAX_TIMEOUT_S)).contains(t))
                .map(|t| t as u32)
                .ok_or(format!(
                    "actions.command.timeoutS : de 1 à {MAX_TIMEOUT_S} secondes"
                ))?,
        };
        Ok(Command {
            program,
            args,
            timeout_s,
        })
    }

    pub fn to_json(&self) -> Value {
        json!({
            "program": self.program,
            "args": self.args,
            "timeoutS": self.timeout_s,
        })
    }
}

/// Vérifie toute une liste (`rules.set`) : nombre et identifiants distincts.
pub fn check_list(rules: &[Rule]) -> Result<(), String> {
    if rules.len() > MAX_RULES {
        return Err(format!("{MAX_RULES} règles au plus"));
    }
    let mut ids: Vec<&str> = rules
        .iter()
        .map(|r| r.id.as_str())
        .filter(|id| !id.is_empty())
        .collect();
    ids.sort_unstable();
    if ids.windows(2).any(|w| w[0] == w[1]) {
        return Err("le même ruleId deux fois".into());
    }
    Ok(())
}

/// Dossier d'une règle : absolu, ou relatif aux parties sûres.
pub fn check_folder(folder: &str) -> Result<(), String> {
    if folder.chars().any(char::is_control) || folder.len() > 4096 {
        return Err("actions.folder : sans caractère de contrôle, 4 096 octets au plus".into());
    }
    if Path::new(folder).is_absolute() {
        return Ok(());
    }
    let parts: Vec<&str> = folder.split('/').collect();
    if parts.len() > MAX_FOLDER_PARTS {
        return Err(format!(
            "actions.folder : {MAX_FOLDER_PARTS} parties au plus sous le dossier des téléchargements"
        ));
    }
    for part in parts {
        if part.is_empty()
            || part == "."
            || part == ".."
            || sanitize(part, NameRules::native()) != part
        {
            return Err(format!(
                "actions.folder : un chemin absolu, ou des noms de dossier valides séparés par \
                 « / » ({part:?} refusé)"
            ));
        }
    }
    Ok(())
}

/// Modèle de nom : variables connues, sans séparateur de dossier ni contrôle.
pub fn check_template(template: &str) -> Result<(), String> {
    if template.trim().is_empty()
        || template.chars().count() > 255
        || template
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
    {
        return Err(
            "actions.rename : 1 à 255 caractères, sans « / », « \\ » ni caractère de contrôle"
                .into(),
        );
    }
    if !check_variables(template, &NAME_VARIABLES) {
        return Err(format!(
            "actions.rename : variable inconnue ou accolade seule (variables : {})",
            NAME_VARIABLES.join(", ")
        ));
    }
    Ok(())
}

/// Nom de domaine : lettres, chiffres, `-`, étiquettes séparées par des points ; ni
/// adresse IP ni joker.
fn is_domain(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 253
        && d.parse::<std::net::IpAddr>().is_err()
        && d.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

fn known_keys(obj: &Map<String, Value>, known: &[&str], what: &str) -> Result<(), String> {
    match obj.keys().find(|k| !known.contains(&k.as_str())) {
        Some(k) => Err(format!("{what} : champ inconnu {k:?}")),
        None => Ok(()),
    }
}

fn strings(value: Option<&Value>, what: &str) -> Result<Vec<String>, String> {
    let list = match value {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(list)) => list,
        Some(_) => return Err(format!("{what} : une liste de textes")),
    };
    if list.len() > MAX_LIST {
        return Err(format!("{what} : {MAX_LIST} valeurs au plus"));
    }
    let mut out: Vec<String> = list
        .iter()
        .map(|v| v.as_str().map(|s| s.trim().to_owned()))
        .collect::<Option<_>>()
        .ok_or(format!("{what} : une liste de textes"))?;
    out.dedup();
    Ok(out)
}

fn optional_text(value: Option<&Value>, what: &str) -> Result<Option<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.trim().to_owned())),
        Some(_) => Err(format!("{what} : un texte")),
    }
}

fn regex_field(value: Option<&Value>, what: &str) -> Result<Option<String>, String> {
    // Une expression garde ses espaces : ils comptent.
    let pattern = match value {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) if s.is_empty() => return Ok(None),
        Some(Value::String(s)) => s.clone(),
        Some(_) => return Err(format!("{what} : un texte")),
    };
    if pattern.chars().count() > MAX_REGEX {
        return Err(format!("{what} : {MAX_REGEX} caractères au plus"));
    }
    compile(&pattern).map_err(|e| format!("{what} : expression refusée ({e})"))?;
    Ok(Some(pattern))
}

#[cfg(test)]
mod tests;
