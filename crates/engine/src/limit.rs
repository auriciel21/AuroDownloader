//! Limitation de débit (SPEC §7.6, F-15) : seaux de jetons emboîtés, global, par file et
//! par tâche.
//!
//! Avant chaque lecture réseau, une connexion attend que chacun de ses seaux soit à jour
//! ([`Throttle::ready`]), puis paie ce qu'elle a reçu ([`Throttle::pay`]). La taille d'une
//! lecture ne se choisit pas (le client HTTP rend les octets par morceaux) : un seau peut
//! donc passer en dette, et la lecture suivante attend qu'elle soit remboursée. Le débit
//! moyen est tenu ; un seau inemployé accumule au plus [`BURST`] de son débit.
//!
//! Les seaux se rechargent en continu, selon le temps écoulé : c'est la limite d'un
//! rechargement toutes les 10 ms de la SPEC, sans minuterie. Un seau sans limite ne coûte
//! qu'une lecture atomique. Changer une limite réveille les connexions qui attendent :
//! elle agit aussitôt, sans couper les connexions.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;

/// Réserve d'un seau inemployé, en durée de son débit : assez pour qu'un réveil en retard
/// (la minuterie de Windows avance par pas d'environ 15 ms) ne fasse pas perdre de débit.
pub const BURST: Duration = Duration::from_millis(100);

/// Attente minimale d'une connexion en dette.
const MIN_WAIT: Duration = Duration::from_millis(1);

/// Un seau de jetons, partagé par les connexions qu'il limite.
pub struct Bucket {
    /// Octets par seconde ; 0 : sans limite.
    rate: AtomicU64,
    state: Mutex<State>,
    changed: Notify,
}

struct State {
    /// Octets disponibles ; négatif : dette.
    tokens: f64,
    last: Instant,
}

impl Bucket {
    /// Seau de `rate` octets par seconde ; `None` : sans limite.
    pub fn new(rate: Option<u64>) -> Arc<Bucket> {
        Arc::new(Bucket {
            rate: AtomicU64::new(rate.unwrap_or(0)),
            state: Mutex::new(State {
                tokens: 0.0,
                last: Instant::now(),
            }),
            changed: Notify::new(),
        })
    }

    /// Limite actuelle, en octets par seconde.
    pub fn rate(&self) -> Option<u64> {
        match self.rate.load(Ordering::Acquire) {
            0 => None,
            rate => Some(rate),
        }
    }

    /// Change la limite ; les connexions qui attendent recalculent aussitôt leur attente.
    /// Une dette reste due, sauf si la limite disparaît.
    pub fn set_rate(&self, rate: Option<u64>) {
        let mut state = self.lock();
        refill(&mut state, self.rate.load(Ordering::Acquire));
        let rate = rate.unwrap_or(0);
        self.rate.store(rate, Ordering::Release);
        state.tokens = match rate {
            0 => 0.0,
            rate => state.tokens.min(capacity(rate)),
        };
        drop(state);
        self.changed.notify_waiters();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Deux nombres toujours cohérents : un verrou empoisonné reste utilisable.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Retire `bytes` octets reçus, quitte à passer en dette.
    pub fn pay(&self, bytes: u64) {
        let rate = self.rate.load(Ordering::Acquire);
        if rate == 0 {
            return;
        }
        let mut state = self.lock();
        refill(&mut state, rate);
        state.tokens -= bytes as f64;
    }

    /// Revient quand le seau n'est plus en dette.
    pub async fn ready(&self) {
        loop {
            // Inscrit avant la lecture de la limite : un changement fait entre les deux
            // réveille quand même cette attente.
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let wait = {
                let rate = self.rate.load(Ordering::Acquire);
                if rate == 0 {
                    return;
                }
                let mut state = self.lock();
                refill(&mut state, rate);
                if state.tokens >= 0.0 {
                    return;
                }
                Duration::from_secs_f64(-state.tokens / rate as f64).max(MIN_WAIT)
            };
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = changed => {}
            }
        }
    }
}

impl fmt::Debug for Bucket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bucket")
            .field("rate", &self.rate())
            .finish()
    }
}

/// Réserve maximale d'un seau de débit `rate` (non nul).
fn capacity(rate: u64) -> f64 {
    (rate as f64 * BURST.as_secs_f64()).max(1.0)
}

/// Ajoute les jetons gagnés depuis le dernier passage, dans la limite de la réserve.
fn refill(state: &mut State, rate: u64) {
    let now = Instant::now();
    if rate == 0 {
        state.tokens = 0.0;
    } else {
        let earned = now.duration_since(state.last).as_secs_f64() * rate as f64;
        state.tokens = (state.tokens + earned).min(capacity(rate));
    }
    state.last = now;
}

/// Les seaux d'une exécution : global, de sa file, de la tâche. Vide : aucune limite.
#[derive(Debug, Clone, Default)]
pub struct Throttle {
    buckets: Vec<Arc<Bucket>>,
}

impl Throttle {
    pub fn new(buckets: impl IntoIterator<Item = Arc<Bucket>>) -> Self {
        Throttle {
            buckets: buckets.into_iter().collect(),
        }
    }

    /// Revient quand aucun seau n'est en dette : l'attente est celle du plus restrictif,
    /// car les autres se rechargent pendant ce temps.
    pub async fn ready(&self) {
        for bucket in &self.buckets {
            bucket.ready().await;
        }
    }

    /// Paie `bytes` octets reçus à chaque seau.
    pub fn pay(&self, bytes: usize) {
        for bucket in &self.buckets {
            bucket.pay(bytes as u64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK: usize = 16 * 1024;

    /// Octets qu'une connexion reçoit en `during`, par morceaux de `CHUNK`.
    async fn receive(throttle: &Throttle, during: Duration) -> u64 {
        let start = Instant::now();
        let mut total = 0;
        while start.elapsed() < during {
            throttle.ready().await;
            throttle.pay(CHUNK);
            total += CHUNK as u64;
        }
        total
    }

    fn assert_near(got: u64, expected: u64) {
        assert_near_for(1, got, expected);
    }

    /// Chaque connexion peut lire un morceau de plus que le débit ne permet : celui qui
    /// met le seau en dette.
    fn assert_near_for(connections: u64, got: u64, expected: u64) {
        let slack = (connections + 1) * CHUNK as u64;
        assert!(
            got + slack >= expected && got <= expected + slack,
            "{got} octets reçus, {expected} attendus"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_unlimited_bucket_never_waits() {
        let throttle = Throttle::new([Bucket::new(None)]);
        let start = Instant::now();
        for _ in 0..1000 {
            throttle.ready().await;
            throttle.pay(CHUNK);
        }
        assert_eq!(start.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn a_bucket_holds_its_rate() {
        let throttle = Throttle::new([Bucket::new(Some(1_000_000))]);
        let got = receive(&throttle, Duration::from_secs(5)).await;
        assert_near(got, 5_000_000);
    }

    #[tokio::test(start_paused = true)]
    async fn the_most_restrictive_bucket_wins() {
        let global = Bucket::new(Some(4_000_000));
        let task = Bucket::new(Some(500_000));
        let throttle = Throttle::new([global, task]);
        let got = receive(&throttle, Duration::from_secs(4)).await;
        assert_near(got, 2_000_000);
    }

    #[tokio::test(start_paused = true)]
    async fn connections_share_their_common_bucket() {
        let global = Bucket::new(Some(1_000_000));
        let a = Throttle::new([Arc::clone(&global), Bucket::new(None)]);
        let b = Throttle::new([Arc::clone(&global)]);
        let during = Duration::from_secs(4);
        let (got_a, got_b) = tokio::join!(receive(&a, during), receive(&b, during));
        assert_near_for(2, got_a + got_b, 4_000_000);
        // Ni l'une ni l'autre n'est affamée.
        assert!(got_a > 1_000_000 && got_b > 1_000_000, "{got_a} et {got_b}");
    }

    #[tokio::test(start_paused = true)]
    async fn an_idle_bucket_saves_at_most_its_burst() {
        let bucket = Bucket::new(Some(1_000_000));
        tokio::time::sleep(Duration::from_secs(60)).await;
        let throttle = Throttle::new([bucket]);
        let got = receive(&throttle, Duration::from_secs(1)).await;
        assert_near(got, 1_000_000 + 100_000);
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_limit_applies_at_once_without_waking_late() {
        // Une dette d'un mégaoctet à un octet par seconde : onze jours d'attente.
        let bucket = Bucket::new(Some(1));
        bucket.pay(1_000_000);
        let waiter = tokio::spawn({
            let bucket = Arc::clone(&bucket);
            async move {
                let start = Instant::now();
                bucket.ready().await;
                start.elapsed()
            }
        });
        tokio::task::yield_now().await;
        bucket.set_rate(None);
        let waited = waiter.await.unwrap();
        assert!(waited < Duration::from_millis(10), "{waited:?}");

        // Une limite abaissée s'applique à la lecture suivante.
        bucket.set_rate(Some(2_000_000));
        let throttle = Throttle::new([Arc::clone(&bucket)]);
        assert_near(receive(&throttle, Duration::from_secs(1)).await, 2_000_000);
        bucket.set_rate(Some(500_000));
        assert_near(receive(&throttle, Duration::from_secs(2)).await, 1_000_000);
        assert_eq!(bucket.rate(), Some(500_000));
    }
}
