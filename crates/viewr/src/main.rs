//! Binary entry point for viewr. All logic lives in the library crate; this file
//! only initializes logging and hands control to [`viewr::run`].

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    // Respect RUST_LOG if set; otherwise stay quiet. viewr never logs user
    // activity, only diagnostics, and only to the local console.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    viewr::run()?;
    Ok(())
}
