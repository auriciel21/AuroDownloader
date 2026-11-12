//! Contrat du protocole (SPEC §9.1, phase 7) : les schémas JSON de `docs/protocol`, un par
//! méthode (paramètres et résultat) et par notification, plus les définitions communes.
//! Chaque réponse et chaque notification vues par les tests du protocole (`rpc::tests`)
//! y sont validées ; une méthode servie ou une notification émise sans schéma fait échouer
//! les tests.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use jsonschema::{Registry, Validator};
use serde_json::{Value, json};

/// Adresse de base des schémas (`$id`), jamais résolue par le réseau.
const BASE: &str = "https://aurodownloader.org/protocol/";

/// Dossier des schémas, à la racine du dépôt.
pub fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/protocol")
}

/// Les schémas d'un dossier (`common.json`, `methods/*.json`, `notifications/*.json`), par
/// chemin relatif.
pub fn load(root: &Path) -> Vec<(String, Value)> {
    let mut found = Vec::new();
    let read = |path: &Path| -> Value {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path:?} : {e}"));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path:?} : {e}"))
    };
    found.push(("common.json".to_owned(), read(&root.join("common.json"))));
    for kind in ["methods", "notifications"] {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(root.join(kind))
            .unwrap_or_else(|e| panic!("{kind} : {e}"))
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        entries.sort();
        for path in entries {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            found.push((format!("{kind}/{name}"), read(&path)));
        }
    }
    found
}

struct Contract {
    registry: Registry<'static>,
    /// Méthodes et notifications qui ont un schéma.
    methods: Vec<String>,
    notifications: Vec<String>,
    validators: Mutex<HashMap<String, Arc<Validator>>>,
}

fn contract() -> &'static Contract {
    static CONTRACT: OnceLock<Contract> = OnceLock::new();
    CONTRACT.get_or_init(|| {
        let schemas = load(&dir());
        let mut registry = Registry::new();
        let (mut methods, mut notifications) = (Vec::new(), Vec::new());
        for (path, schema) in schemas {
            let id = schema["$id"].as_str().unwrap_or_default().to_owned();
            assert_eq!(id, format!("{BASE}{path}"), "$id de {path}");
            if let Some(name) = path.strip_prefix("methods/") {
                methods.push(name.trim_end_matches(".json").to_owned());
            } else if let Some(name) = path.strip_prefix("notifications/") {
                notifications.push(name.trim_end_matches(".json").to_owned());
            }
            registry = registry
                .add(id, schema)
                .unwrap_or_else(|e| panic!("{path} : {e}"));
        }
        Contract {
            registry: registry.prepare().expect("schémas du protocole"),
            methods,
            notifications,
            validators: Mutex::default(),
        }
    })
}

/// Le validateur de la définition `pointer` (`methods/task.add.json#/$defs/result`…).
fn validator(pointer: &str) -> Arc<Validator> {
    let contract = contract();
    let mut cache = contract.validators.lock().unwrap();
    if let Some(v) = cache.get(pointer) {
        return Arc::clone(v);
    }
    let built = jsonschema::options()
        .with_registry(&contract.registry)
        .build(&json!({"$ref": format!("{BASE}{pointer}")}))
        .unwrap_or_else(|e| panic!("schéma {pointer} : {e}"));
    let built = Arc::new(built);
    cache.insert(pointer.to_owned(), Arc::clone(&built));
    built
}

fn check(pointer: &str, what: &str, value: &Value) {
    // Couverture : `AURO_CONTRACT_LOG=<fichier>` y note chaque définition validée.
    if let Some(path) = std::env::var_os("AURO_CONTRACT_LOG") {
        use std::io::Write as _;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{pointer}");
        }
    }
    let errors: Vec<String> = validator(pointer)
        .iter_errors(value)
        .map(|e| format!("{} : {e}", e.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "{what} hors du contrat ({pointer}) :\n{}\n{value}",
        errors.join("\n")
    );
}

/// Méthodes qui ont un schéma.
pub fn methods() -> &'static [String] {
    &contract().methods
}

/// Notifications qui ont un schéma.
pub fn notifications() -> &'static [String] {
    &contract().notifications
}

/// Valide les paramètres d'un appel.
pub fn check_params(method: &str, params: &Value) {
    let pointer = format!("methods/{method}.json#/$defs/params");
    check(&pointer, &format!("paramètres de {method}"), params);
}

/// Valide une réponse à `method` : son résultat, ou son erreur. Une méthode inconnue
/// (refusée par le moteur) n'a pas de schéma.
pub fn check_reply(method: &str, reply: &Value) {
    if let Some(error) = reply.get("error") {
        check("common.json#/$defs/rpcError", "erreur", error);
    } else if methods().iter().any(|m| m == method) {
        let pointer = format!("methods/{method}.json#/$defs/result");
        check(&pointer, &format!("résultat de {method}"), &reply["result"]);
    } else {
        panic!("{method} répond sans schéma dans docs/protocol/methods");
    }
}

/// Valide une notification du moteur.
pub fn check_notification(message: &Value) {
    let method = message["method"].as_str().unwrap_or_default();
    assert!(
        notifications().iter().any(|n| n == method),
        "notification {method} sans schéma dans docs/protocol/notifications"
    );
    let pointer = format!("notifications/{method}.json#/$defs/params");
    check(
        &pointer,
        &format!("notification {method}"),
        &message["params"],
    );
}

/// Les méthodes servies (bras de `match` de `methods.rs` qui commencent par leur nom).
pub fn served_methods() -> Vec<String> {
    let mut served = Vec::new();
    for line in include_str!("methods.rs").lines() {
        let line = line.trim_start().trim_start_matches("| ");
        let Some(rest) = line.strip_prefix('"') else {
            continue;
        };
        let Some((name, after)) = rest.split_once('"') else {
            continue;
        };
        let arm = after.trim_start().starts_with("=>") || after.trim_start().starts_with('|');
        let dotted = name.split_once('.').is_some_and(|(a, b)| {
            !a.is_empty()
                && !b.is_empty()
                && a.bytes().all(|c| c.is_ascii_lowercase())
                && b.bytes().all(|c| c.is_ascii_alphabetic())
        });
        if arm && dotted && !served.iter().any(|s| s == name) {
            served.push(name.to_owned());
        }
    }
    served
}

/// Les notifications émises (`notification("…"` de `session.rs`).
pub fn emitted_notifications() -> Vec<String> {
    let source = include_str!("session.rs");
    let mut found: Vec<String> = Vec::new();
    for part in source.split("notification(").skip(1) {
        let part = part.trim_start();
        if let Some(rest) = part.strip_prefix('"')
            && let Some((name, _)) = rest.split_once('"')
            && !found.iter().any(|f| f == name)
        {
            found.push(name.to_owned());
        }
    }
    found
}

/// Rôle d'une définition : ce que le client envoie (paramètres), ou ce qu'il reçoit
/// (résultats, notifications). Une définition commune peut avoir les deux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Role {
    Request,
    Response,
}

fn types_of(schema: &Value) -> Option<Vec<String>> {
    match schema.get("type")? {
        Value::String(t) => Some(vec![t.clone()]),
        Value::Array(list) => Some(
            list.iter()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect(),
        ),
        _ => None,
    }
}

fn strings(schema: &Value, key: &str) -> Vec<String> {
    schema
        .get(key)
        .and_then(Value::as_array)
        .map(|list| list.iter().map(|v| v.to_string()).collect())
        .unwrap_or_default()
}

/// Compare une définition de la version figée (`old`) à la courante (`new`), selon son
/// rôle ; les écarts qui casseraient un client de la version figée vont dans `errors`.
fn compare(old: &Value, new: &Value, role: Role, path: &str, errors: &mut Vec<String>) {
    let mut fail = |what: String| errors.push(format!("{path} : {what}"));
    if let Some(target) = old.get("$ref") {
        if new.get("$ref") != Some(target) {
            fail(format!("référence {target} changée"));
        }
        return;
    }
    match (types_of(old), types_of(new)) {
        (Some(o), Some(n)) => {
            let (wider, narrower, what) = match role {
                Role::Request => (&n, &o, "type accepté retiré"),
                Role::Response => (&o, &n, "type nouveau dans une réponse"),
            };
            if let Some(t) = narrower.iter().find(|t| !wider.contains(t)) {
                fail(format!("{what} : {t}"));
            }
        }
        (Some(o), None) if role == Role::Response => fail(format!("type {o:?} plus garanti")),
        (None, Some(n)) if role == Role::Request => fail(format!("type {n:?} désormais exigé")),
        _ => {}
    }
    // Valeurs acceptées d'un paramètre : toutes gardées. (Une réponse peut en gagner : un
    // client ignore une valeur qu'il ne connaît pas, SPEC 6.1.)
    if role == Role::Request {
        let new_values = strings(new, "enum");
        if new.get("enum").is_some()
            && let Some(v) = strings(old, "enum")
                .iter()
                .find(|v| !new_values.contains(v))
        {
            fail(format!("valeur {v} plus acceptée"));
        }
    }
    let (old_required, new_required) = (strings(old, "required"), strings(new, "required"));
    match role {
        Role::Request => {
            if let Some(r) = new_required.iter().find(|r| !old_required.contains(r)) {
                fail(format!("paramètre {r} devenu obligatoire"));
            }
        }
        Role::Response => {
            if let Some(r) = old_required.iter().find(|r| !new_required.contains(r)) {
                fail(format!("champ {r} plus toujours présent"));
            }
        }
    }
    if let Some(props) = old.get("properties").and_then(Value::as_object) {
        for (name, old_prop) in props {
            match new.pointer(&format!(
                "/properties/{}",
                name.replace('~', "~0").replace('/', "~1")
            )) {
                Some(new_prop) => {
                    compare(old_prop, new_prop, role, &format!("{path}.{name}"), errors)
                }
                None => errors.push(format!("{path} : champ {name} retiré")),
            }
        }
    }
    if let Some(old_items) = old.get("items") {
        match new.get("items") {
            Some(new_items) => compare(old_items, new_items, role, &format!("{path}[]"), errors),
            None => errors.push(format!("{path} : éléments plus décrits")),
        }
    }
    for key in ["anyOf", "allOf", "oneOf"] {
        if let Some(old_list) = old.get(key).and_then(Value::as_array) {
            match new.get(key).and_then(Value::as_array) {
                Some(new_list) if new_list.len() == old_list.len() => {
                    for (i, (o, n)) in old_list.iter().zip(new_list).enumerate() {
                        compare(o, n, role, &format!("{path}.{key}[{i}]"), errors);
                    }
                }
                _ => errors.push(format!("{path} : {key} changé")),
            }
        }
    }
}

/// Définitions communes atteintes par `schema` (références vers `common.json`).
fn common_refs(schema: &Value, found: &mut Vec<String>) {
    match schema {
        Value::Object(map) => {
            if let Some(target) = map.get("$ref").and_then(Value::as_str)
                && let Some(name) = target.strip_prefix(&format!("{BASE}common.json#/$defs/"))
            {
                found.push(name.to_owned());
            }
            map.values().for_each(|v| common_refs(v, found));
        }
        Value::Array(items) => items.iter().for_each(|v| common_refs(v, found)),
        _ => {}
    }
}

/// Écarts entre une version figée des schémas (`old`) et la courante (`new`) qui
/// casseraient un client de la version figée (SPEC 6.1, 9.1).
fn breaking_changes(old: &[(String, Value)], new: &[(String, Value)]) -> Vec<String> {
    let mut errors = Vec::new();
    let find = |list: &[(String, Value)], path: &str| {
        list.iter().find(|(p, _)| p == path).map(|(_, v)| v.clone())
    };
    let old_common = find(old, "common.json").unwrap_or_default();
    let new_common = find(new, "common.json").unwrap_or_default();
    // Rôles des définitions communes, d'après leurs usages dans la version figée.
    let mut roles: HashMap<String, std::collections::HashSet<Role>> = HashMap::new();
    let mut pending: Vec<(String, Role)> = Vec::new();
    for (path, schema) in old {
        let defs = [("params", path.starts_with("methods/")), ("result", false)];
        for (def, is_request) in defs {
            let Some(body) = schema.pointer(&format!("/$defs/{def}")) else {
                continue;
            };
            let role = if is_request {
                Role::Request
            } else {
                Role::Response
            };
            let mut found = Vec::new();
            common_refs(body, &mut found);
            pending.extend(found.into_iter().map(|name| (name, role)));
        }
    }
    while let Some((name, role)) = pending.pop() {
        if roles.entry(name.clone()).or_default().insert(role) {
            let mut found = Vec::new();
            if let Some(body) = old_common.pointer(&format!("/$defs/{name}")) {
                common_refs(body, &mut found);
            }
            pending.extend(found.into_iter().map(|n| (n, role)));
        }
    }
    for (path, schema) in old {
        let Some(current) = find(new, path) else {
            errors.push(format!("{path} : retiré"));
            continue;
        };
        if path == "common.json" {
            continue;
        }
        let pairs = [("params", path.starts_with("methods/")), ("result", false)];
        for (def, is_request) in pairs {
            if let Some(o) = schema.pointer(&format!("/$defs/{def}")) {
                let role = if is_request {
                    Role::Request
                } else {
                    Role::Response
                };
                match current.pointer(&format!("/$defs/{def}")) {
                    Some(n) => compare(o, n, role, &format!("{path}#{def}"), &mut errors),
                    None => errors.push(format!("{path}#{def} : retiré")),
                }
            }
        }
    }
    let mut names: Vec<&String> = roles.keys().collect();
    names.sort();
    for name in names {
        let Some(o) = old_common.pointer(&format!("/$defs/{name}")) else {
            continue;
        };
        let Some(n) = new_common.pointer(&format!("/$defs/{name}")) else {
            errors.push(format!("common.json#{name} : retiré"));
            continue;
        };
        for role in &roles[name] {
            compare(o, n, *role, &format!("common.json#{name}"), &mut errors);
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    /// « Un client de la version précédente doit toujours fonctionner » (SPEC 9.1) : les
    /// schémas courants ne cassent rien de ceux de la version 1, figés dans
    /// `docs/protocol/v1`. Une nouvelle version majeure du protocole s'y fige à son tour.
    #[test]
    fn the_protocol_keeps_compatible_with_version_1() {
        let old = load(&dir().join("v1"));
        let new = load(&dir());
        let errors = breaking_changes(&old, &new);
        assert!(
            errors.is_empty(),
            "changements incompatibles :\n{}",
            errors.join("\n")
        );
    }

    #[test]
    fn breaking_changes_are_found() {
        let method = |params: Value, result: Value| {
            vec![
                ("common.json".to_owned(), json!({"$defs": {}})),
                (
                    "methods/x.y.json".to_owned(),
                    json!({"$defs": {"params": params, "result": result}}),
                ),
            ]
        };
        let base = method(
            json!({"type": "object", "properties": {"a": {"type": "string"}, "k": {"enum": ["u", "v"]}}, "required": ["a"]}),
            json!({"type": "object", "properties": {"id": {"type": "string"}, "n": {"type": ["integer", "null"]}}, "required": ["id", "n"]}),
        );
        assert!(breaking_changes(&base, &base).is_empty());
        // Ajouts permis : paramètre facultatif, champ de réponse, type accepté, valeur.
        let fine = method(
            json!({"type": "object", "properties": {"a": {"type": ["string", "null"]}, "b": {"type": "string"}, "k": {"enum": ["u", "v", "w"]}}, "required": ["a"]}),
            json!({"type": "object", "properties": {"id": {"type": "string"}, "n": {"type": "integer"}, "z": {"type": "boolean"}}, "required": ["id", "n", "z"]}),
        );
        assert!(
            breaking_changes(&base, &fine).is_empty(),
            "{:?}",
            breaking_changes(&base, &fine)
        );
        let broken = [
            // Paramètre devenu obligatoire.
            method(
                json!({"type": "object", "properties": {"a": {"type": "string"}, "k": {"enum": ["u", "v"]}, "b": {"type": "string"}}, "required": ["a", "b"]}),
                base[1].1["$defs"]["result"].clone(),
            ),
            // Valeur plus acceptée.
            method(
                json!({"type": "object", "properties": {"a": {"type": "string"}, "k": {"enum": ["u"]}}, "required": ["a"]}),
                base[1].1["$defs"]["result"].clone(),
            ),
            // Champ de réponse retiré.
            method(
                base[1].1["$defs"]["params"].clone(),
                json!({"type": "object", "properties": {"n": {"type": ["integer", "null"]}}, "required": ["n"]}),
            ),
            // Champ de réponse devenu facultatif.
            method(
                base[1].1["$defs"]["params"].clone(),
                json!({"type": "object", "properties": {"id": {"type": "string"}, "n": {"type": ["integer", "null"]}}, "required": ["id"]}),
            ),
            // Type changé dans une réponse.
            method(
                base[1].1["$defs"]["params"].clone(),
                json!({"type": "object", "properties": {"id": {"type": "integer"}, "n": {"type": ["integer", "null"]}}, "required": ["id", "n"]}),
            ),
            // Méthode retirée.
            vec![("common.json".to_owned(), json!({"$defs": {}}))],
        ];
        for (i, new) in broken.iter().enumerate() {
            assert!(!breaking_changes(&base, new).is_empty(), "cas {i}");
        }
    }

    #[test]
    fn every_served_method_and_emitted_notification_has_a_schema() {
        let served = served_methods();
        assert!(served.len() > 40, "{} méthodes", served.len());
        let missing: Vec<&String> = served.iter().filter(|m| !methods().contains(m)).collect();
        assert!(missing.is_empty(), "méthodes sans schéma : {missing:?}");
        let extra: Vec<&String> = methods().iter().filter(|m| !served.contains(m)).collect();
        assert!(
            extra.is_empty(),
            "schémas de méthodes non servies : {extra:?}"
        );
        let emitted = emitted_notifications();
        assert!(emitted.len() > 10, "{} notifications", emitted.len());
        let missing: Vec<&String> = emitted
            .iter()
            .filter(|n| !notifications().contains(n))
            .collect();
        assert!(
            missing.is_empty(),
            "notifications sans schéma : {missing:?}"
        );
    }

    #[test]
    fn every_schema_builds() {
        for method in methods() {
            validator(&format!("methods/{method}.json#/$defs/params"));
            validator(&format!("methods/{method}.json#/$defs/result"));
        }
        for name in notifications() {
            validator(&format!("notifications/{name}.json#/$defs/params"));
        }
        // Un résultat qui manque à son contrat est refusé.
        let bad = std::panic::catch_unwind(|| {
            check_reply("task.add", &json!({"result": {"id": 3}}));
        });
        assert!(bad.is_err());
    }
}
