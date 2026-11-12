//! wasmtime (Cranelift, compilation en code machine) : fuel, `StoreLimits`, pile bornée.

use wasmtime::{
    Caller, Config, Engine, Error, Instance, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, TypedFunc,
};

use crate::{Host, Limits};

pub const NAME: &str = "wasmtime 50";

struct State {
    host: Host,
    limits: StoreLimits,
}

pub struct Runtime {
    engine: Engine,
    module: Module,
    linker: Linker<State>,
}

fn memory_of(caller: &mut Caller<'_, State>) -> Result<Memory, Error> {
    caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| Error::msg("pas de mémoire exportée"))
}

fn read(caller: &mut Caller<'_, State>, ptr: i32, len: i32) -> Result<Vec<u8>, Error> {
    let memory = memory_of(caller)?;
    let len = usize::try_from(len).map_err(|_| Error::msg("taille négative"))?;
    if len > 1 << 20 {
        return Err(Error::msg("message trop long"));
    }
    let mut bytes = vec![0; len];
    memory.read(&*caller, ptr as u32 as usize, &mut bytes)?;
    Ok(bytes)
}

impl Runtime {
    pub fn new(wasm: &[u8], limits: &Limits) -> Result<Self, String> {
        let mut config = Config::new();
        config.consume_fuel(true).max_wasm_stack(limits.stack);
        let engine = Engine::new(&config).map_err(|e| e.to_string())?;
        let module = Module::new(&engine, wasm).map_err(|e| e.to_string())?;
        let mut linker = Linker::new(&engine);
        linker
            .func_wrap(
                "auro",
                "fetch",
                |mut caller: Caller<'_, State>, ptr: i32, len: i32| -> Result<i64, Error> {
                    let request = read(&mut caller, ptr, len)?;
                    let answer = caller.data_mut().host.fetch(&request);
                    let alloc: TypedFunc<i32, i32> = caller
                        .get_export("auro_alloc")
                        .and_then(|e| e.into_func())
                        .ok_or_else(|| Error::msg("pas d'auro_alloc"))?
                        .typed(&caller)?;
                    let at = alloc.call(&mut caller, answer.len() as i32)?;
                    memory_of(&mut caller)?.write(&mut caller, at as u32 as usize, &answer)?;
                    Ok((i64::from(at as u32) << 32) | answer.len() as i64)
                },
            )
            .map_err(|e| e.to_string())?;
        linker
            .func_wrap(
                "auro",
                "log",
                |mut caller: Caller<'_, State>, ptr: i32, len: i32| -> Result<(), Error> {
                    let text = read(&mut caller, ptr, len)?;
                    caller.data_mut().host.log(&text);
                    Ok(())
                },
            )
            .map_err(|e| e.to_string())?;
        Ok(Runtime {
            engine,
            module,
            linker,
        })
    }

    fn store(&self, host: Host, limits: &Limits) -> Store<State> {
        let state = State {
            host,
            limits: StoreLimitsBuilder::new()
                .memory_size(limits.memory)
                .instances(1)
                .build(),
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limits);
        store.set_fuel(limits.fuel).expect("fuel activé");
        store
    }

    fn finish<T>(
        store: Store<State>,
        result: Result<T, String>,
        limits: &Limits,
    ) -> (Result<T, String>, Host, u64) {
        let used = limits.fuel - store.get_fuel().unwrap_or(0);
        (result, store.into_data().host, used)
    }

    fn instance(&self, store: &mut Store<State>) -> Result<Instance, String> {
        self.linker
            .instantiate(&mut *store, &self.module)
            .map_err(trap)
    }

    pub fn resolve(
        &self,
        host: Host,
        input: &[u8],
        limits: &Limits,
    ) -> (Result<Vec<u8>, String>, Host, u64) {
        let mut store = self.store(host, limits);
        let result = (|| {
            let instance = self.instance(&mut store)?;
            let e = trap;
            let alloc = instance
                .get_typed_func::<i32, i32>(&mut store, "auro_alloc")
                .map_err(e)?;
            let resolve = instance
                .get_typed_func::<(i32, i32), i64>(&mut store, "auro_resolve")
                .map_err(e)?;
            let memory = instance
                .get_memory(&mut store, "memory")
                .ok_or("pas de mémoire")?;
            let at = alloc.call(&mut store, input.len() as i32).map_err(e)?;
            memory
                .write(&mut store, at as u32 as usize, input)
                .map_err(|e| e.to_string())?;
            let packed = resolve
                .call(&mut store, (at, input.len() as i32))
                .map_err(e)? as u64;
            let (ptr, len) = ((packed >> 32) as usize, (packed & 0xffff_ffff) as usize);
            let mut output = vec![0; len];
            memory
                .read(&store, ptr, &mut output)
                .map_err(|e| e.to_string())?;
            Ok(output)
        })();
        Self::finish(store, result, limits)
    }

    pub fn call_i32(
        &self,
        host: Host,
        name: &str,
        limits: &Limits,
    ) -> (Result<u64, String>, Host, u64) {
        let mut store = self.store(host, limits);
        let result = (|| {
            let instance = self.instance(&mut store)?;
            let f = instance
                .get_typed_func::<(), i32>(&mut store, name)
                .map_err(|e| e.to_string())?;
            f.call(&mut store, ())
                .map(|v| u64::from(v as u32))
                .map_err(trap)
        })();
        Self::finish(store, result, limits)
    }

    pub fn call_deep(
        &self,
        host: Host,
        depth: u64,
        limits: &Limits,
    ) -> (Result<u64, String>, Host, u64) {
        let mut store = self.store(host, limits);
        let result = (|| {
            let instance = self.instance(&mut store)?;
            let f = instance
                .get_typed_func::<i64, i64>(&mut store, "auro_deep")
                .map_err(|e| e.to_string())?;
            f.call(&mut store, depth as i64)
                .map(|v| v as u64)
                .map_err(trap)
        })();
        Self::finish(store, result, limits)
    }
}

/// Le code d'arrêt d'une erreur d'exécution (`OutOfFuel`…), sinon son premier message.
fn trap(e: Error) -> String {
    match e.downcast_ref::<wasmtime::Trap>() {
        Some(trap) => format!("Trap({trap:?})"),
        None => format!("{e:#}"),
    }
}
