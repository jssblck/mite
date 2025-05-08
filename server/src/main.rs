//! Entrypoint for the server.

use color_eyre::Result;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    Ok(())
}
