//! Build script for the crate.

fn main() {
    // Only includes the test environment variables for non-release builds.
    // Ideally we'd do `cfg!(test)` for even more safety,
    // but this then breaks Rust Analyzer's macro evaluation.
    if cfg!(debug_assertions) {
        include_test_envs();
    }
}

/// Includes specific environment variables from the enclosing environment
/// (filled from the .env file, if present) into the build.
///
/// All variables are prefixed with `TEST_` in an attempt to make it more
/// clear that these should not be going into release builds.
///
/// The intention here is to enable compiling tests using environment variables;
/// since we generate test cases at compile time it is awkward to need
/// runtime values filled in for some test cases.
fn include_test_envs() {
    if let Ok(path) = dotenvy::dotenv() {
        println!("cargo::rerun-if-changed={}", path.display());
    }

    let envs = ["GOOGLE_CLOUD_TOKEN"];
    for env in envs {
        println!("cargo::rerun-if-env-changed={env}");
        if let Ok(value) = std::env::var(env) {
            println!("cargo::rustc-env=TEST_{env}={value}");
        }
    }
}
