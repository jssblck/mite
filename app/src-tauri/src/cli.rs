//! Thin wrappers around the installed mite CLI: version, doctor, init-config.
//!
//! All invocations set the working directory to the mite home so the CLI
//! resolves `mite.toml`, `models\`, and `cache\engines\` exactly as it does for
//! a developer running it by hand, and point `MITE_GPU_RUNTIME_DIR` at the home
//! so doctor reports the same GPU pack the app manages.

use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::home;

/// Apply Windows process flags that keep a console window from flashing when we
/// spawn the CLI from a GUI app.
fn quiet(mut cmd: Command) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// A `mite` command pre-configured with the home as CWD and the GPU runtime dir
/// exported (and prepended to PATH so the OS loader finds the DLLs).
pub fn command() -> Result<Command> {
    let exe = home::cli_exe()?;
    let home_dir = home::mite_home()?;
    let gpu = home::gpu_runtime_dir()?;
    let mut cmd = quiet(Command::new(exe));
    cmd.current_dir(&home_dir).env("MITE_GPU_RUNTIME_DIR", &gpu);
    let gpu_str = gpu.to_string_lossy().to_string();
    let path = std::env::var("PATH").unwrap_or_default();
    let new_path = if path.is_empty() {
        gpu_str
    } else {
        format!("{gpu_str};{path}")
    };
    cmd.env("PATH", new_path);
    Ok(cmd)
}

/// The installed CLI version reported by `mite --version`, or `None` when the
/// binary is absent or fails to run.
pub fn installed_version() -> Option<String> {
    let exe = home::cli_exe().ok()?;
    if !exe.exists() {
        return None;
    }
    let output = quiet(Command::new(exe)).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // clap prints "mite <version>"; keep the version token.
    text.split_whitespace().last().map(str::to_string)
}

/// Run `mite doctor --json` and return the parsed readiness report.
pub fn doctor_json() -> Result<serde_json::Value> {
    let output = command()?
        .arg("doctor")
        .arg("--json")
        .output()
        .context("running mite doctor")?;
    if !output.status.success() {
        bail!(
            "mite doctor failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("parsing doctor JSON")
}

/// Write a default `mite.toml` into the home via the CLI's own init-config, so
/// the config always matches the CLI's current defaults. The default backend is
/// the TensorRT -> CUDA -> CPU chain, which runs on CPU when no GPU pack is
/// present and uses the GPU automatically once it is.
pub fn write_default_config() -> Result<()> {
    let status = command()?
        .arg("init-config")
        .arg("--force")
        .status()
        .context("running mite init-config")?;
    if !status.success() {
        bail!("mite init-config failed");
    }
    Ok(())
}
