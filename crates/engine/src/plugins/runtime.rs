//! Exécution d'un plugin par wasmi (SPEC §7.20) : une instance neuve par résolution,
//! limites de calcul, de mémoire et de pile ; les seuls imports sont `auro.fetch` et
//! `auro.log`, servis par l'hôte. Code bloquant : à lancer hors de l'exécuteur de tokio.

use wasmi::{
    Caller, Config, Engine, Error, Instance, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, TrapCode,
};

/// Limites d'une résolution.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Unités de calcul de wasmi (environ 3·10⁹ par seconde sur la machine du prototype).
    pub fuel: u64,
    /// Mémoire linéaire, en octets.
    pub memory: usize,
    /// Appels imbriqués (la limite par défaut de wasmi, 1 000, est trop courte).
    pub depth: usize,
}

pub const LIMITS: Limits = Limits {
    fuel: 5_000_000_000,
    memory: 64 << 20,
    depth: 10_000,
};

/// Message échangé avec le plugin, au plus (requête de `fetch`, entrée, journal) ; la
/// réponse de `fetch` (page) peut aller jusqu'à 8 Mio, la sortie de `auro_resolve` aussi.
const MAX_MESSAGE: usize = 1 << 20;
const MAX_OUTPUT: usize = 8 << 20;
/// Réponse d'`auro.fetch` remise au plugin, encodée en JSON, au plus (audit D12).
const MAX_ANSWER: usize = 12 << 20;
/// Lignes de journal gardées par résolution.
const MAX_LOGS: usize = 50;
/// Entrées d'une table de fonctions, au plus, déclarées ou agrandies (revue de sécurité,
/// étape 5) : sans elle, une table démesurée ferait allouer au moteur des Gio. Un plugin
/// compilé par Rust en a quelques centaines.
const MAX_TABLE_ELEMENTS: usize = 100_000;

/// Ce que l'hôte offre à une instance.
pub type Fetch = Box<dyn FnMut(&[u8]) -> Vec<u8> + Send>;

struct State {
    fetch: Fetch,
    logs: Vec<String>,
    limits: StoreLimits,
}

fn engine(limits: &Limits) -> Engine {
    let mut config = Config::default();
    config
        .consume_fuel(true)
        .set_max_recursion_depth(limits.depth)
        // Bornes de la lecture du module (fonctions, globales, segments, paramètres,
        // taille moyenne d'une fonction), recommandées par wasmi contre un module
        // malveillant : la compilation aussi a ses limites (audit D8).
        .enforced_limits(wasmi::EnforcedLimits::strict());
    Engine::new(&config)
}

fn memory_of(caller: &Caller<'_, State>) -> Result<Memory, Error> {
    caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| Error::new("pas de mémoire exportée"))
}

fn read(caller: &Caller<'_, State>, ptr: i32, len: i32) -> Result<Vec<u8>, Error> {
    let len = usize::try_from(len).map_err(|_| Error::new("taille négative"))?;
    if len > MAX_MESSAGE {
        return Err(Error::new("message trop long"));
    }
    let mut bytes = vec![0; len];
    memory_of(caller)?
        .read(caller, ptr as u32 as usize, &mut bytes)
        .map_err(|_| Error::new("adresse hors de la mémoire"))?;
    Ok(bytes)
}

fn linker(engine: &Engine) -> Result<Linker<State>, String> {
    let mut linker = Linker::new(engine);
    linker
        .func_wrap(
            "auro",
            "fetch",
            |mut caller: Caller<'_, State>, ptr: i32, len: i32| -> Result<i64, Error> {
                let request = read(&caller, ptr, len)?;
                let mut answer = (caller.data_mut().fetch)(&request);
                // Une réponse de 8 Mio d'octets de contrôle grossit jusqu'à six fois une
                // fois échappée en JSON : bornée avant d'entrer dans la mémoire du plugin
                // (audit D12).
                if answer.len() > MAX_ANSWER {
                    answer = br#"{"error": "reponse trop grosse une fois encodee"}"#.to_vec();
                }
                let alloc = caller
                    .get_export("auro_alloc")
                    .and_then(|e| e.into_func())
                    .ok_or_else(|| Error::new("pas d'auro_alloc"))?
                    .typed::<i32, i32>(&caller)?;
                let at = alloc.call(&mut caller, answer.len() as i32)?;
                memory_of(&caller)?
                    .write(&mut caller, at as u32 as usize, &answer)
                    .map_err(|_| Error::new("zone rendue par auro_alloc hors de la mémoire"))?;
                Ok((i64::from(at as u32) << 32) | answer.len() as i64)
            },
        )
        .map_err(|e| e.to_string())?;
    linker
        .func_wrap(
            "auro",
            "log",
            |mut caller: Caller<'_, State>, ptr: i32, len: i32| -> Result<(), Error> {
                let text = read(&caller, ptr, len.min(4096))?;
                let logs = &mut caller.data_mut().logs;
                if logs.len() < MAX_LOGS {
                    let text: String = String::from_utf8_lossy(&text)
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(500)
                        .collect();
                    logs.push(text);
                }
                Ok(())
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(linker)
}

/// Un arrêt du plugin, dit sans jargon.
fn stopped(e: &Error) -> String {
    match e.as_trap_code() {
        Some(TrapCode::OutOfFuel) => "le plugin a dépassé sa limite de calcul".into(),
        Some(TrapCode::StackOverflow) => "le plugin a dépassé sa limite de pile".into(),
        Some(TrapCode::GrowthOperationLimited) => "le plugin a dépassé sa limite de mémoire".into(),
        Some(code) => format!("le plugin s'est arrêté ({code:?})"),
        None => format!("le plugin s'est arrêté : {e}"),
    }
}

fn store(engine: &Engine, fetch: Fetch, limits: &Limits) -> Store<State> {
    let state = State {
        fetch,
        logs: Vec::new(),
        limits: StoreLimitsBuilder::new()
            .memory_size(limits.memory)
            .table_elements(MAX_TABLE_ELEMENTS)
            .instances(1)
            .memories(1)
            .tables(4)
            .build(),
    };
    let mut store = Store::new(engine, state);
    store.limiter(|s| &mut s.limits);
    store.set_fuel(limits.fuel).expect("fuel activé");
    store
}

fn instantiate(
    bytes: &[u8],
    fetch: Fetch,
    limits: &Limits,
) -> Result<(Store<State>, Instance), String> {
    // Avant toute instanciation : rien ne s'exécute hors de `auro_resolve`.
    super::manifest::refuse_start(bytes)?;
    let engine = engine(limits);
    let module = Module::new(&engine, bytes).map_err(|e| format!("WebAssembly invalide : {e}"))?;
    for import in module.imports() {
        if import.module() != "auro" || !matches!(import.name(), "fetch" | "log") {
            return Err(format!(
                "import non permis : {}.{} (seuls auro.fetch et auro.log)",
                import.module(),
                import.name()
            ));
        }
    }
    let linker = linker(&engine)?;
    let mut store = store(&engine, fetch, limits);
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| stopped(&e))?;
    Ok((store, instance))
}

/// Vérifie qu'un fichier est un plugin que ce moteur sait servir : WebAssembly valide,
/// imports permis, exports de l'interface avec leurs types.
pub fn check(bytes: &[u8]) -> Result<(), String> {
    let refuse: Fetch = Box::new(|_| {
        r#"{"error": "pas de requête pendant la vérification"}"#
            .as_bytes()
            .to_vec()
    });
    let (mut store, instance) = instantiate(bytes, refuse, &LIMITS)?;
    instance
        .get_memory(&store, "memory")
        .ok_or("export manquant : memory")?;
    instance
        .get_typed_func::<i32, i32>(&mut store, "auro_alloc")
        .map_err(|_| "export manquant ou mal typé : auro_alloc(i32) -> i32")?;
    instance
        .get_typed_func::<(i32, i32), i64>(&mut store, "auro_resolve")
        .map_err(|_| "export manquant ou mal typé : auro_resolve(i32, i32) -> i64")?;
    Ok(())
}

/// Ce qu'une résolution a donné.
#[derive(Debug)]
pub struct Run {
    /// Sortie de `auro_resolve` (JSON attendu, non lu ici), ou l'arrêt du plugin.
    pub output: Result<Vec<u8>, String>,
    /// Journal du plugin (`auro.log`).
    pub logs: Vec<String>,
}

/// Lance `auro_resolve` sur `input`, dans une instance neuve.
pub fn resolve(bytes: &[u8], input: &[u8], fetch: Fetch, limits: &Limits) -> Run {
    let (mut store, instance) = match instantiate(bytes, fetch, limits) {
        Ok(pair) => pair,
        Err(e) => {
            return Run {
                output: Err(e),
                logs: Vec::new(),
            };
        }
    };
    let output = (|| {
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or("export manquant : memory")?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, "auro_alloc")
            .map_err(|_| "export manquant : auro_alloc")?;
        let run = instance
            .get_typed_func::<(i32, i32), i64>(&mut store, "auro_resolve")
            .map_err(|_| "export manquant : auro_resolve")?;
        let len = i32::try_from(input.len()).map_err(|_| "entrée trop longue")?;
        let at = alloc.call(&mut store, len).map_err(|e| stopped(&e))?;
        memory
            .write(&mut store, at as u32 as usize, input)
            .map_err(|_| "zone rendue par auro_alloc hors de la mémoire")?;
        let packed = run.call(&mut store, (at, len)).map_err(|e| stopped(&e))? as u64;
        let (ptr, len) = ((packed >> 32) as usize, (packed & 0xffff_ffff) as usize);
        if len > MAX_OUTPUT {
            return Err(format!("sortie de plus de {} Mio", MAX_OUTPUT >> 20));
        }
        let mut output = vec![0; len];
        memory
            .read(&store, ptr, &mut output)
            .map_err(|_| "sortie hors de la mémoire du plugin")?;
        Ok::<_, String>(output)
    })();
    let logs = std::mem::take(&mut store.data_mut().logs);
    Run { output, logs }
}
