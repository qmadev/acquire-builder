use std::collections::HashMap;
use std::fs::create_dir_all;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};

use crate::platform::{Platform, get_platform};

pub fn create_acquire_project(out_path: &Path) -> Result<PathBuf> {
    let toml = include_str!("../acquire-runner/Cargo.toml");
    let lock = include_str!("../acquire-runner/Cargo.lock");
    let aes = include_str!("../acquire-runner/src/aes_stream.rs");
    let pystandalone = include_str!("../acquire-runner/src/pystandalone.rs");
    let interpreter = include_str!("../acquire-runner/src/interpreter.rs");
    let main = include_str!("../acquire-runner/src/main.rs");

    let root_files = vec![
        ("Cargo.toml", toml),
        ("Cargo.lock", lock),
        ("src/aes_stream.rs", aes),
        ("src/pystandalone.rs", pystandalone),
        ("src/interpreter.rs", interpreter),
        ("src/main.rs", main),
    ];

    let acquire_project = out_path.join("acquire-runner");
    match create_dir_all(acquire_project.join("src").as_path()) {
        Ok(_) => (),
        Err(e) => match e.kind() {
            std::io::ErrorKind::AlreadyExists => (),
            _ => return Err(anyhow!(e)),
        },
    }

    for (filename, data) in root_files {
        let out_path = acquire_project.join(filename);
        std::fs::write(out_path, data)?;
    }

    Ok(acquire_project)
}

pub fn compile_acquire(build_path: &Path) -> Result<()> {
    let project_path = build_path.join("acquire-runner");
    let pyo3_config_path = build_path.join("pyo3-build-config.txt").canonicalize()?;
    let debug = log::log_enabled!(log::Level::Debug);
    let dist = get_platform();
    let mut envs: HashMap<String, String> = HashMap::new();
    envs.insert(
        "PYO3_CONFIG_FILE".into(),
        pyo3_config_path.display().to_string(),
    );

    match dist {
        Platform::Windows => {
            envs.insert("RUSTFLAGS".into(), "-C target-feature=+crt-static".into())
        }
        Platform::Linux => None,
        _ => envs.insert(
            "RUSTFLAGS".into(),
            "-C link-arg=-undefined -C link-arg=dynamic_lookup".into(),
        ),
    };

    let compile = Command::new("cargo")
        .current_dir(project_path)
        .envs(&envs)
        .stdout(if debug { Stdio::inherit() } else { Stdio::null() })
        .stderr(if debug { Stdio::inherit() } else { Stdio::null() })
        .args(["build", "--locked", "--release"])
        .output()
        .context("Failed to execute cargo build command")?;

    if !compile.status.success() {
        return Err(anyhow!("Failed to compile acquire-runner binary"));
    }

    Ok(())
}
