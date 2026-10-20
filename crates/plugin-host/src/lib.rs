//! The sandbox a plugin runs in, shared by the program and the office server.
//!
//! A seat runs an office's plugins itself. A copy of Excalibur View that isn't
//! allowed to run code it downloads (the Mac App Store edition) asks its office
//! server to run them instead, and the server runs them here, in exactly the
//! same interpreter with exactly the same limits, so a plugin says the same
//! thing wherever it runs.
//!
//! A module that imports anything at all is refused: there is nothing for it
//! to import. It gets a budget of work and of memory, and running out of either
//! is an error, not a hang.

use plugin_api::{Answer, Input, Manifest, Output};

/// How much work one run may do. About a minute of a busy interpreter —
/// far more than reading a hundred sheets needs, and a stop for one that
/// has gone round in circles.
pub const FUEL: u64 = 60_000_000_000;

/// How much memory one run may hold.
pub const MEMORY: usize = 1536 << 20;

/// The most a plugin may hand back.
const MOST_OUTPUT: usize = 64 << 20;

// ---- the sandbox --------------------------------------------------------------

struct Host {
    limits: wasmi::StoreLimits,
}

fn instantiate(wasm: &[u8], fuel: u64) -> Result<(wasmi::Store<Host>, wasmi::Instance), String> {
    let mut config = wasmi::Config::default();
    config.consume_fuel(true);
    let engine = wasmi::Engine::new(&config);
    let module = wasmi::Module::new(&engine, wasm)
        .map_err(|e| format!("The plugin could not be read as WebAssembly: {e}"))?;
    // There is nothing to import. A plugin that asks for something wants to
    // reach outside the sandbox, and does not run.
    let wants: Vec<String> = module
        .imports()
        .map(|i| format!("{}.{}", i.module(), i.name()))
        .collect();
    if !wants.is_empty() {
        return Err(format!(
            "The plugin asks for things Excalibur View does not give plugins: {}.",
            wants.join(", ")
        ));
    }
    let limits = wasmi::StoreLimitsBuilder::new()
        .memory_size(MEMORY)
        .instances(1)
        .memories(1)
        .tables(4)
        .build();
    let mut store = wasmi::Store::new(&engine, Host { limits });
    store.limiter(|host| &mut host.limits);
    store
        .set_fuel(fuel)
        .map_err(|e| format!("The plugin's budget could not be set: {e}"))?;
    let linker = wasmi::Linker::<Host>::new(&engine);
    // WHAT: makes the plugin's instance and runs its start function, in one call.
    // WHY: wasmi 1.x joined the two steps (`instantiate` then `start`) into this one.
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| explain(&e))?;
    Ok((store, instance))
}

fn explain(e: &wasmi::Error) -> String {
    let text = e.to_string();
    if text.contains("fuel") {
        "The plugin took too long and was stopped.".into()
    } else if text.contains("memory") && (text.contains("limit") || text.contains("grow")) {
        "The plugin wanted more memory than it is allowed and was stopped.".into()
    } else {
        format!("The plugin stopped with an error: {text}")
    }
}

/// Reads an answer out of the plugin's memory: address high, length low.
fn take(store: &wasmi::Store<Host>, memory: &wasmi::Memory, packed: i64) -> Result<Vec<u8>, String> {
    let at = ((packed as u64) >> 32) as usize;
    let len = (packed as u64 & 0xffff_ffff) as usize;
    if len > MOST_OUTPUT {
        return Err("The plugin answered with more than Excalibur View will read.".into());
    }
    let data = memory.data(store);
    data.get(at..at + len)
        .map(|b| b.to_vec())
        .ok_or_else(|| "The plugin answered from outside its own memory.".into())
}

/// What a plugin says about itself.
pub fn manifest_of(wasm: &[u8]) -> Result<Manifest, String> {
    let (mut store, instance) = instantiate(wasm, FUEL)?;
    let memory = instance
        .get_memory(&store, "memory")
        .ok_or("The plugin has no memory to talk through.")?;
    let manifest = instance
        .get_typed_func::<(), i64>(&store, "hv_manifest")
        .map_err(|_| "The plugin does not say what it is (no hv_manifest).".to_string())?;
    let packed = manifest.call(&mut store, ()).map_err(|e| explain(&e))?;
    let bytes = take(&store, &memory, packed)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("The plugin's description could not be read: {e}"))
}

/// Runs one command. Blocking — call it off the window's thread.
pub fn run(wasm: &[u8], input: &Input) -> Result<Output, String> {
    run_with_fuel(wasm, input, FUEL)
}

/// [`run`] with a budget of work other than the usual one.
pub fn run_with_fuel(wasm: &[u8], input: &Input, fuel: u64) -> Result<Output, String> {
    let (mut store, instance) = instantiate(wasm, fuel)?;
    let memory = instance
        .get_memory(&store, "memory")
        .ok_or("The plugin has no memory to talk through.")?;
    let alloc = instance
        .get_typed_func::<i32, i32>(&store, "hv_alloc")
        .map_err(|_| "The plugin cannot be handed anything (no hv_alloc).".to_string())?;
    let go = instance
        .get_typed_func::<(i32, i32), i64>(&store, "hv_run")
        .map_err(|_| "The plugin cannot be run (no hv_run).".to_string())?;
    let bytes = plugin_api::pack(input);
    let len = i32::try_from(bytes.len()).map_err(|_| "There is too much to hand the plugin.".to_string())?;
    let at = alloc.call(&mut store, len).map_err(|e| explain(&e))?;
    memory
        .write(&mut store, at as u32 as usize, &bytes)
        .map_err(|_| "The plugin gave no room for what it was handed.".to_string())?;
    let packed = go.call(&mut store, (at, len)).map_err(|e| explain(&e))?;
    let answer = take(&store, &memory, packed)?;
    match serde_json::from_slice::<Answer>(&answer) {
        Ok(Answer::Done(output)) => Ok(output),
        Ok(Answer::Failed(why)) => Err(why),
        Err(e) => Err(format!("The plugin's answer could not be read: {e}")),
    }
}

