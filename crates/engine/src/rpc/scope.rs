//! Portées d'une connexion (SPEC §6.9) : ce qu'un client a le droit de demander.
//!
//! Sur le canal local, réservé par le système à l'utilisateur (SPEC §6.2), une connexion a
//! d'emblée tous les droits (`admin`). Un client peut en abandonner à l'ouverture
//! (`engine.hello`, paramètre `scope`) : le relais de l'extension navigateur se limite
//! ainsi à `add`, et le moteur lui refuse tout le reste (décision du 2026-11-07). Une
//! portée ne s'élargit jamais. Les jetons des transports réseau viendront avec
//! l'interface distante (F-39).

use serde_json::Value;

/// Une portée : ce qu'elle autorise est décrit par [`Scope::allows`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Lire : listes, détails, abonnements.
    Read,
    /// Ajouter des téléchargements, et rien d'autre.
    Add,
    /// `add` et `read`, plus agir sur les tâches et les files.
    Control,
    /// Tout, dont l'arrêt du moteur et les actions de fin (veille, extinction).
    Admin,
}

impl Scope {
    pub fn parse(text: &str) -> Option<Scope> {
        match text {
            "read" => Some(Scope::Read),
            "add" => Some(Scope::Add),
            "control" => Some(Scope::Control),
            "admin" => Some(Scope::Admin),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Add => "add",
            Scope::Control => "control",
            Scope::Admin => "admin",
        }
    }

    /// Une connexion de cette portée peut-elle faire ce que `need` demande ? Vaut aussi
    /// pour passer à la portée `need`, qui ne doit rien ajouter.
    pub fn allows(self, need: Scope) -> bool {
        match self {
            Scope::Admin => true,
            Scope::Control => need != Scope::Admin,
            Scope::Read | Scope::Add => need == self,
        }
    }
}

/// Portée qu'exige une méthode ; `None` pour `engine.hello`, permise à tous, et pour les
/// méthodes inconnues, qui répondent « méthode inconnue ».
pub fn required(method: &str, params: &Value) -> Option<Scope> {
    Some(match method {
        "engine.hello" => return None,
        "task.get" | "task.list" | "queue.list" | "limits.get" | "schedule.list"
        | "settings.get" | "events.subscribe" | "events.unsubscribe" | "sync.snapshot"
        | "engine.stats"
        | "history.list"
        // État des outils de la vidéo (SPEC §7.17).
        | "media.tools" => Scope::Read,
        "task.add" | "task.addBatch" | "media.probe" | "linkscan.run"
        // Fichiers Metalink (SPEC §7.7) : lus, au besoin à une adresse, puis ajoutés.
        | "metalink.read" | "metalink.add"
        // Tailles des liens analysés : une requête vers chaque adresse (SPEC §7.18).
        | "linkscan.probe"
        // Plugins d'hébergeurs (SPEC §7.20) : s'en servir à l'ajout, jamais en installer.
        | "plugin.match"
        | "plugin.resolve"
        | "plugin.add" => Scope::Add,
        // Attacher une planification peut éteindre la machine (SPEC §7.11).
        "queue.create" | "queue.update" if params.get("scheduleId").is_some() => Scope::Admin,
        "task.pause"
        | "task.resume"
        | "task.cancel"
        | "task.remove"
        | "task.restart"
        | "task.update"
        | "task.move"
        | "task.addSource"
        | "task.setFiles"
        | "queue.create"
        | "queue.update"
        | "queue.delete"
        | "limits.set"
        | "engine.cancelEndAction"
        | "engine.setNetwork"
        // Effacer l'historique ne touche ni aux tâches ni aux fichiers (F-35).
        | "history.clear" => Scope::Control,
        "engine.shutdown" | "schedule.set" | "schedule.delete" | "settings.set" => Scope::Admin,
        // Identifiants de sites (SPEC §7.14) : même la liste (noms d'utilisateur).
        "login.list" | "login.set" | "login.delete" => Scope::Admin,
        // Télécharge et installe des programmes (SPEC §7.17).
        "media.installTools" => Scope::Admin,
        // Interfaces réseau de la machine, pour régler `torrent.interface` (SPEC §7.16).
        "torrent.interfaces" => Scope::Admin,
        // Jetons de l'interface distante (SPEC §7.19) : un jeton n'a jamais `admin`, ces
        // méthodes ne s'atteignent donc que par le canal local.
        "token.create" | "token.list" | "token.revoke" | "remote.status" => Scope::Admin,
        // Diagnostic (N-12) : nettoyé, mais tout l'état du moteur.
        "engine.diagnostic" => Scope::Admin,
        // Installer, voir les permissions, activer, retirer (SPEC §7.20).
        "plugin.inspect" | "plugin.install" | "plugin.list" | "plugin.remove"
        | "plugin.enable" => Scope::Admin,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn scopes_never_widen() {
        use Scope::*;
        let all = [Read, Add, Control, Admin];
        for s in all {
            assert!(Admin.allows(s));
            assert!(s.allows(s));
            assert_eq!(Scope::parse(s.as_str()), Some(s));
        }
        assert!(Control.allows(Read) && Control.allows(Add) && !Control.allows(Admin));
        assert!(!Add.allows(Read) && !Add.allows(Control));
        assert!(!Read.allows(Add) && !Read.allows(Control));
        assert_eq!(Scope::parse("root"), None);
    }

    #[test]
    fn each_method_has_its_scope() {
        let none = json!({});
        assert_eq!(required("engine.hello", &none), None);
        assert_eq!(required("task.explode", &none), None);
        assert_eq!(required("task.add", &none), Some(Scope::Add));
        assert_eq!(required("sync.snapshot", &none), Some(Scope::Read));
        assert_eq!(required("task.pause", &none), Some(Scope::Control));
        assert_eq!(required("queue.update", &none), Some(Scope::Control));
        assert_eq!(
            required("queue.update", &json!({"scheduleId": null})),
            Some(Scope::Admin),
            "même pour détacher"
        );
        assert_eq!(required("schedule.set", &none), Some(Scope::Admin));
        assert_eq!(required("engine.shutdown", &none), Some(Scope::Admin));
        assert_eq!(required("torrent.interfaces", &none), Some(Scope::Admin));
        for method in ["token.create", "token.list", "token.revoke"] {
            assert_eq!(required(method, &none), Some(Scope::Admin), "{method}");
        }
    }

    /// Revue de sécurité (étape 5) : une méthode absente de [`required`] serait permise à
    /// toute portée. Chaque méthode servie par `methods.rs` (bras de `match` qui commencent
    /// par son nom) doit donc y avoir la sienne, sauf `engine.hello`.
    #[test]
    fn every_served_method_has_a_scope() {
        let served = super::super::contract::served_methods();
        assert!(served.len() > 40, "{} méthodes trouvées", served.len());
        let none = json!({});
        let missing: Vec<&String> = served
            .iter()
            .filter(|m| *m != "engine.hello" && required(m, &none).is_none())
            .collect();
        assert!(missing.is_empty(), "sans portée : {missing:?}");
    }
}
