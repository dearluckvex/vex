use anyhow::Result;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    tracing::info!("XTune CLI - Linux Router Proxy Client");
    tracing::info!("Usage: xtune-cli <config.yaml>");

    // Phase 6: implement CLI mode
    eprintln!("CLI mode is under development. Please use the GUI for now.");

    Ok(())
}
