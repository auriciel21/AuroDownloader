//! `auro-relay` : hôte de messagerie native de l'extension navigateur (SPEC §4.5).
//!
//! Lancé par le navigateur, jamais à la main : sa sortie standard est le canal de
//! l'extension, rien d'autre ne doit y être écrit. Les erreurs vont sur la sortie
//! d'erreur, que le navigateur garde dans son journal. Le navigateur passe en argument
//! l'origine de l'extension (Chrome, Edge) ou le chemin du manifeste et l'identifiant de
//! l'extension (Firefox) : le relais ne s'en sert pas, le manifeste ayant déjà limité les
//! extensions admises.

use std::process::ExitCode;

use auro_cli::relay;
use auro_engine::paths;

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("auro-relay : {e}");
            return ExitCode::FAILURE;
        }
    };
    let endpoint = paths::endpoint();
    match runtime.block_on(relay::serve(
        tokio::io::stdin(),
        tokio::io::stdout(),
        &endpoint,
    )) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("auro-relay : {e}");
            ExitCode::FAILURE
        }
    }
}
