//! Coffre du système (SPEC §5.1 et §8.2) : les en-têtes secrets d'une tâche (`Cookie`,
//! `Authorization`, `Proxy-Authorization`) y sont rangés ; la base ne garde qu'une
//! référence (`task_headers.secret_ref`).
//!
//! - Windows : Gestionnaire d'identification (identifiants génériques de l'utilisateur,
//!   gardés sur cette machine). Un identifiant tient au plus 2 560 octets : un secret plus
//!   long est réparti sur plusieurs, numérotés.
//! - Linux : Secret Service (trousseau de la session : GNOME Keyring, KWallet…). Sans lui
//!   (serveur sans session graphique, WSL), la mémoire du moteur.
//! - Ailleurs (Android : Keystore, phase 4) : la mémoire du moteur.
//!
//! En mémoire, un secret ne survit pas à l'arrêt du moteur : une tâche reprise ensuite
//! part sans cet en-tête (`transfer`).
//!
//! Chaque base a son espace de noms (réglage `vault.namespace`) : un moteur d'essai ne
//! touche jamais aux secrets du moteur réel. Ordre : le secret est écrit avant sa
//! référence en base, et la référence effacée avant le secret ; un arrêt brutal entre les
//! deux laisse au pire un secret orphelin, que le moteur efface à son démarrage
//! (`scheduler`, ramassage).
//!
//! Les appels au coffre bloquent (D-Bus sous Linux) : les exécutions, le ramassage et
//! les effacements passent par `tokio::task::spawn_blocking`. Seule l'écriture d'un ajout
//! se fait dans la boucle du planificateur, pour que le secret et sa référence soient
//! enregistrés dans le même tour (voir le ramassage).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

/// Taille maximale d'un secret, en octets.
pub const MAX_SECRET: usize = 32 * 1024;

/// Le coffre a refusé ou n'a pas répondu. Le message ne cite jamais le secret.
#[derive(Debug, Clone, thiserror::Error)]
#[error("coffre du système : {0}")]
pub struct VaultError(pub String);

/// Échec de l'effacement des secrets d'une base ([`Vault::erase_base`]).
#[derive(Debug, thiserror::Error)]
pub enum EraseError {
    #[error(transparent)]
    Store(#[from] crate::store::StoreError),
    #[error(transparent)]
    Vault(#[from] VaultError),
}

/// Un coffre : écrit, lit, efface et énumère les secrets d'un espace de noms.
pub trait Backend: Send + Sync {
    /// Nom du coffre, pour le journal.
    fn kind(&self) -> &'static str;
    fn put(&self, namespace: &str, id: &str, secret: &[u8]) -> Result<(), VaultError>;
    /// `None` : aucun secret sous ce nom.
    fn get(&self, namespace: &str, id: &str) -> Result<Option<Vec<u8>>, VaultError>;
    /// Sans erreur si le secret n'existe pas.
    fn delete(&self, namespace: &str, id: &str) -> Result<(), VaultError>;
    /// Identifiants des secrets de l'espace de noms.
    fn ids(&self, namespace: &str) -> Result<Vec<String>, VaultError>;
}

/// Poignée du coffre, limitée à un espace de noms ; se clone librement.
#[derive(Clone)]
pub struct Vault {
    backend: Arc<dyn Backend>,
    namespace: String,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vault")
            .field("kind", &self.backend.kind())
            .field("namespace", &self.namespace)
            .finish()
    }
}

/// La mémoire du moteur : ce que reçoivent les tests et le mode `direct`.
impl Default for Vault {
    fn default() -> Self {
        Vault::memory()
    }
}

impl Vault {
    /// Secrets en mémoire, perdus à l'arrêt du moteur.
    pub fn memory() -> Vault {
        Vault {
            backend: Arc::new(Memory::default()),
            namespace: String::new(),
        }
    }

    /// Le coffre du système (voir le module), sinon la mémoire.
    pub fn system() -> Vault {
        #[cfg(windows)]
        let backend: Arc<dyn Backend> = Arc::new(windows::CredentialManager);
        #[cfg(target_os = "linux")]
        let backend: Arc<dyn Backend> = Arc::new(linux::SecretService::default());
        #[cfg(not(any(windows, target_os = "linux")))]
        let backend: Arc<dyn Backend> = Arc::new(Memory::default());
        Vault {
            backend,
            namespace: String::new(),
        }
    }

    /// Un coffre donné (tests : coffre en panne).
    #[cfg(test)]
    pub(crate) fn with_backend(backend: Arc<dyn Backend>) -> Vault {
        Vault {
            backend,
            namespace: String::new(),
        }
    }

    /// Le même coffre, limité à l'espace de noms `namespace`.
    pub fn scoped(&self, namespace: &str) -> Vault {
        Vault {
            backend: Arc::clone(&self.backend),
            namespace: namespace.to_owned(),
        }
    }

    pub fn kind(&self) -> &'static str {
        self.backend.kind()
    }

    /// Range `secret` ; renvoie sa référence.
    pub fn put(&self, secret: &str) -> Result<String, VaultError> {
        if secret.len() > MAX_SECRET {
            return Err(VaultError(format!(
                "secret trop long : {} octets, {MAX_SECRET} au plus",
                secret.len()
            )));
        }
        let id = uuid::Uuid::now_v7().simple().to_string();
        self.backend.put(&self.namespace, &id, secret.as_bytes())?;
        Ok(id)
    }

    /// Le secret de la référence `id` ; `None` s'il n'existe plus.
    pub fn get(&self, id: &str) -> Result<Option<String>, VaultError> {
        self.backend
            .get(&self.namespace, id)?
            .map(|bytes| {
                String::from_utf8(bytes)
                    .map_err(|_| VaultError(format!("secret {id} illisible (UTF-8)")))
            })
            .transpose()
    }

    pub fn delete(&self, id: &str) -> Result<(), VaultError> {
        self.backend.delete(&self.namespace, id)
    }

    /// Références de tous les secrets de l'espace de noms.
    pub fn ids(&self) -> Result<Vec<String>, VaultError> {
        self.backend.ids(&self.namespace)
    }

    /// Désinstallation (« Supprimer aussi les données », audit C4) : efface du coffre tous
    /// les secrets de la base `db`, c'est-à-dire de son espace de noms, sans toucher à ceux
    /// d'une autre base. Base absente : rien. Le moteur doit être arrêté (verrou de la
    /// base). Rend le nombre de secrets effacés.
    pub fn erase_base(&self, db: &std::path::Path) -> Result<usize, EraseError> {
        if !db.exists() {
            return Ok(0);
        }
        let _lock = crate::store::lock::acquire(db)?;
        let conn = crate::store::open(db)?;
        let namespace = crate::store::settings::vault_namespace(&conn)?;
        // Un espace de noms vide désignerait les secrets de toutes les bases.
        if namespace.is_empty() {
            return Err(EraseError::Vault(VaultError("espace de noms vide".into())));
        }
        let scoped = self.scoped(&namespace);
        let ids = scoped.ids()?;
        for id in &ids {
            scoped.delete(id)?;
        }
        Ok(ids.len())
    }

    /// Efface les secrets `ids` hors de la boucle appelante ; une erreur est sans gravité
    /// (le ramassage du prochain démarrage les retrouvera).
    pub fn forget(&self, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        let vault = self.clone();
        tokio::task::spawn_blocking(move || {
            for id in ids {
                if let Err(e) = vault.delete(&id) {
                    tracing::warn!(secret = %id, erreur = %e, "secret non effacé");
                }
            }
        });
    }
}

/// Secrets en mémoire.
#[derive(Default)]
pub(crate) struct Memory {
    secrets: Mutex<HashMap<(String, String), Vec<u8>>>,
}

impl Memory {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), Vec<u8>>> {
        self.secrets.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Backend for Memory {
    fn kind(&self) -> &'static str {
        "mémoire du moteur"
    }

    fn put(&self, namespace: &str, id: &str, secret: &[u8]) -> Result<(), VaultError> {
        self.map()
            .insert((namespace.to_owned(), id.to_owned()), secret.to_vec());
        Ok(())
    }

    fn get(&self, namespace: &str, id: &str) -> Result<Option<Vec<u8>>, VaultError> {
        Ok(self
            .map()
            .get(&(namespace.to_owned(), id.to_owned()))
            .cloned())
    }

    fn delete(&self, namespace: &str, id: &str) -> Result<(), VaultError> {
        self.map().remove(&(namespace.to_owned(), id.to_owned()));
        Ok(())
    }

    fn ids(&self, namespace: &str) -> Result<Vec<String>, VaultError> {
        Ok(self
            .map()
            .keys()
            .filter(|(ns, _)| ns == namespace)
            .map(|(_, id)| id.clone())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audit C4 : la désinstallation efface les secrets de la base, et seulement eux ; une
    /// base absente n'est pas créée.
    #[test]
    fn erasing_a_base_removes_its_secrets_only() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("auro.db");
        let vault = Vault::memory();
        assert_eq!(vault.erase_base(&db).unwrap(), 0);
        assert!(!db.exists(), "base absente : pas créée");
        let conn = crate::store::open(&db).unwrap();
        let mine = vault.scoped(&crate::store::settings::vault_namespace(&conn).unwrap());
        drop(conn);
        mine.put("session=1").unwrap();
        mine.put("mot de passe").unwrap();
        let other = vault.scoped("une-autre-base");
        let kept = other.put("à garder").unwrap();
        assert_eq!(vault.erase_base(&db).unwrap(), 2);
        assert!(mine.ids().unwrap().is_empty());
        assert_eq!(other.ids().unwrap(), [kept]);
    }

    /// Écrit, relit, énumère et efface dans `vault`, limité à un espace de noms neuf.
    pub(super) fn round_trip(vault: &Vault) {
        let namespace = format!("essai-{}", uuid::Uuid::now_v7().simple());
        let vault = vault.scoped(&namespace);
        let other = vault.scoped(&format!("{namespace}-autre"));
        assert!(vault.ids().unwrap().is_empty());
        let short = vault.put("session=abc; theme=sombre").unwrap();
        // Plus long qu'un identifiant Windows (2 560 octets), et pas un multiple.
        let long_text: String = (0..9000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let long = vault.put(&long_text).unwrap();
        // Exactement deux morceaux.
        let exact_text = "é".repeat(2560);
        let exact = vault.put(&exact_text).unwrap();
        assert_eq!(
            vault.get(&short).unwrap().as_deref(),
            Some("session=abc; theme=sombre")
        );
        assert_eq!(vault.get(&long).unwrap(), Some(long_text));
        assert_eq!(vault.get(&exact).unwrap(), Some(exact_text));
        let mut ids = vault.ids().unwrap();
        ids.sort();
        let mut expected = vec![short.clone(), long.clone(), exact.clone()];
        expected.sort();
        assert_eq!(ids, expected);
        assert!(other.ids().unwrap().is_empty());
        assert_eq!(other.get(&short).unwrap(), None);
        for id in [&short, &long, &exact] {
            vault.delete(id).unwrap();
            assert_eq!(vault.get(id).unwrap(), None);
        }
        vault.delete(&short).unwrap();
        assert!(vault.ids().unwrap().is_empty());
    }

    #[test]
    fn the_memory_vault_keeps_namespaces_apart() {
        round_trip(&Vault::memory());
    }

    #[test]
    fn an_oversized_secret_is_refused() {
        let vault = Vault::memory();
        assert!(vault.put(&"x".repeat(MAX_SECRET + 1)).is_err());
        assert!(vault.ids().unwrap().is_empty());
    }

    /// Le Gestionnaire d'identification de l'utilisateur : un espace de noms d'essai, vidé
    /// à la fin.
    #[cfg(windows)]
    #[test]
    fn the_windows_credential_manager_round_trips() {
        round_trip(&Vault::system());
    }

    /// Secret Service : seulement si la session en a un (`AURO_SECRET_SERVICE=1`) ; la CI
    /// et WSL n'en ont pas, et le coffre retomberait sur la mémoire.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_secret_service_round_trips() {
        if std::env::var_os("AURO_SECRET_SERVICE").is_none() {
            return;
        }
        let vault = Vault::system();
        round_trip(&vault);
        assert_eq!(vault.kind(), "Secret Service");
    }
}
