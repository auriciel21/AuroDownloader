//! Secret Service (trousseau de la session : GNOME Keyring, KWallet…), par D-Bus.
//!
//! Chaque secret est un élément de la collection par défaut, reconnu par ses attributs
//! (`application`, `namespace`, `id`). Le moteur ne déverrouille jamais un trousseau :
//! cela ouvrirait une fenêtre de mot de passe au nom d'un processus sans interface. Un
//! trousseau verrouillé est une erreur (la tâche réessaiera plus tard).
//!
//! Sans Secret Service à la première utilisation (pas de bus de session, aucun
//! fournisseur), le moteur se rabat sur sa mémoire jusqu'à son arrêt, et le dit dans son
//! journal. Une connexion est ouverte à chaque opération : elles sont rares (un ajout, un
//! départ de tâche, un ramassage).

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use secret_service::EncryptionType;
use secret_service::blocking::SecretService as Service;

use super::{Backend, Memory, VaultError};

const APPLICATION: &str = "org.aurodownloader";

#[derive(Default)]
pub(super) struct SecretService {
    /// `None` : pas encore essayé ; `Some(true)` : Secret Service ; `Some(false)` : la
    /// mémoire.
    usable: Mutex<Option<bool>>,
    memory: Memory,
}

fn failed(what: &str, e: secret_service::Error) -> VaultError {
    VaultError(format!("Secret Service, {what} : {e}"))
}

fn attributes<'a>(namespace: &'a str, id: Option<&'a str>) -> HashMap<&'a str, &'a str> {
    let mut map = HashMap::from([("application", APPLICATION), ("namespace", namespace)]);
    if let Some(id) = id {
        map.insert("id", id);
    }
    map
}

impl SecretService {
    /// Le service, ou `None` s'il faut se rabattre sur la mémoire.
    fn service(&self) -> Result<Option<Service<'static>>, VaultError> {
        let mut usable = self.usable.lock().unwrap_or_else(PoisonError::into_inner);
        if *usable == Some(false) {
            return Ok(None);
        }
        match Service::connect(EncryptionType::Dh) {
            Ok(service) => {
                *usable = Some(true);
                Ok(Some(service))
            }
            Err(e) if usable.is_none() => {
                eprintln!(
                    "aurod : pas de Secret Service ({e}) ; les cookies et jetons des tâches \
                     restent en mémoire, perdus à l'arrêt du moteur"
                );
                *usable = Some(false);
                Ok(None)
            }
            Err(e) => Err(failed("connexion", e)),
        }
    }
}

impl Backend for SecretService {
    fn kind(&self) -> &'static str {
        match *self.usable.lock().unwrap_or_else(PoisonError::into_inner) {
            Some(false) => self.memory.kind(),
            _ => "Secret Service",
        }
    }

    fn put(&self, namespace: &str, id: &str, secret: &[u8]) -> Result<(), VaultError> {
        let Some(service) = self.service()? else {
            return self.memory.put(namespace, id, secret);
        };
        let collection = service
            .get_default_collection()
            .map_err(|e| failed("collection par défaut", e))?;
        if collection
            .is_locked()
            .map_err(|e| failed("collection", e))?
        {
            return Err(VaultError("Secret Service : trousseau verrouillé".into()));
        }
        collection
            .create_item(
                "AuroDownloader : en-tête d'un téléchargement",
                attributes(namespace, Some(id)),
                secret,
                true,
                "text/plain",
            )
            .map_err(|e| failed("écriture", e))?;
        Ok(())
    }

    fn get(&self, namespace: &str, id: &str) -> Result<Option<Vec<u8>>, VaultError> {
        let Some(service) = self.service()? else {
            return self.memory.get(namespace, id);
        };
        let found = service
            .search_items(attributes(namespace, Some(id)))
            .map_err(|e| failed("recherche", e))?;
        match (found.unlocked.first(), found.locked.is_empty()) {
            (Some(item), _) => item
                .get_secret()
                .map(Some)
                .map_err(|e| failed("lecture", e)),
            (None, true) => Ok(None),
            (None, false) => Err(VaultError("Secret Service : trousseau verrouillé".into())),
        }
    }

    fn delete(&self, namespace: &str, id: &str) -> Result<(), VaultError> {
        let Some(service) = self.service()? else {
            return self.memory.delete(namespace, id);
        };
        let found = service
            .search_items(attributes(namespace, Some(id)))
            .map_err(|e| failed("recherche", e))?;
        if !found.locked.is_empty() {
            return Err(VaultError("Secret Service : trousseau verrouillé".into()));
        }
        for item in &found.unlocked {
            item.delete().map_err(|e| failed("effacement", e))?;
        }
        Ok(())
    }

    fn ids(&self, namespace: &str) -> Result<Vec<String>, VaultError> {
        let Some(service) = self.service()? else {
            return self.memory.ids(namespace);
        };
        let found = service
            .search_items(attributes(namespace, None))
            .map_err(|e| failed("recherche", e))?;
        let mut ids = Vec::new();
        for item in found.unlocked.iter().chain(&found.locked) {
            let attrs = item.get_attributes().map_err(|e| failed("attributs", e))?;
            if let Some(id) = attrs.get("id") {
                ids.push(id.clone());
            }
        }
        Ok(ids)
    }
}
