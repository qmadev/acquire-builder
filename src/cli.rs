use std::fs::{File, create_dir};
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use clap_verbosity::{InfoLevel, Verbosity};
use log::{LevelFilter, info};
use walkdir::WalkDir;

use crate::compile::{compile_acquire, create_acquire_project};
use crate::platform::{Platform, get_platform};
use crate::python::dependencies::PythonEnvBuilder;

#[derive(Subcommand)]
enum Commands {
    /// Compile new standalone binaries.
    Compile,

    /// Download standalone binaries from GitHub release.
    Download(DownloadArgs),
}

#[derive(Parser)]
struct DownloadArgs {
    /// GitHub release tag to use.
    #[arg(long, short, default_value = "latest")]
    release: Option<String>,
}

#[derive(Parser)]
struct Args {
    /// Mode to use.
    #[command(subcommand)]
    command: Commands,

    /// Path to your custom acquire config file.
    #[arg(long, short, global = true)]
    config_file: Option<PathBuf>,

    /// Path to store output files.
    #[arg(long, short, global = true, default_value = "build")]
    output_dir: Option<PathBuf>,

    /// Acquire version to use. Will use the latest version as default.
    #[arg(long, global = true)]
    acquire_version: Option<String>,

    /// Dissect version to use. Will use the latest version as default.
    #[arg(long, global = true)]
    dissect_version: Option<String>,

    /// Path to local python executable.
    #[arg(long, short, global = true)]
    python_exe: Option<PathBuf>,

    #[command(flatten)]
    verbose: Verbosity<InfoLevel>,
}

pub fn run() -> Result<()> {
    let args = Args::parse();
    init_logger(args.verbose.log_level_filter());

    // Bail early if needed.
    if let Some(config_file) = &args.config_file
        && !config_file.exists()
    {
        bail!("Failed to find config file: {}", config_file.display())
    }

    if let Some(python_exe) = &args.python_exe
        && !python_exe.exists()
    {
        bail!(
            "Failed to find python interpreter: {}",
            python_exe.display()
        )
    }

    let mut python_env_builder = PythonEnvBuilder::new(
        args.output_dir.clone(),
        args.config_file,
        args.acquire_version,
        args.dissect_version,
        args.python_exe,
    );

    let build_path = python_env_builder.get_build_path();
    let dist = get_platform();

    match args.command {
        Commands::Compile => {
            python_env_builder.assemble(None)?;
            let mut deps = open_deps_zip(build_path.as_path())?;
            let bin_path = create_bin_path(build_path.as_path())?;
            let target_triple = dist.target_triple();

            let acquire_exe = if let Platform::Windows = dist {
                format!("acquire-{}.exe", target_triple)
            } else {
                format!("acquire-{}", target_triple)
            };

            let acquire_exe_path = bin_path.join(acquire_exe);
            let mut acquire = File::create(acquire_exe_path.as_path())?;

            let acquire_project = create_acquire_project(
                args.output_dir
                    .clone()
                    .unwrap_or(PathBuf::from("build"))
                    .as_path(),
            )?;
            compile_acquire(build_path.as_path())?;
            info!("Acquire standalone binary compilation completed");

            let compiled_file = if let Platform::Windows = dist {
                "target/release/acquire.exe"
            } else {
                "target/release/acquire"
            };

            let mut embedded_python =
                File::open(acquire_project.join(compiled_file)).context(format!(
                    "Failed to find {}",
                    acquire_project.join(compiled_file).display()
                ))?;

            std::io::copy(&mut embedded_python, &mut acquire)?;
            std::io::copy(&mut deps, &mut acquire)?;
        }

        Commands::Download(args) => {
            python_env_builder.assemble(args.release)?;
            let bin_path = create_bin_path(build_path.as_path())?;
            let release_bin_path = python_env_builder.get_dist_path().join("pre-compiled");

            for entry in WalkDir::new(release_bin_path).into_iter().flatten() {
                if entry.path().is_dir() {
                    continue;
                }

                let filename = entry.file_name();
                let target = Platform::from_filename(filename.display().to_string());

                if target != dist {
                    python_env_builder.assemble_for_target(target)?
                }

                let mut deps = open_deps_zip(build_path.as_path())?;
                let mut acquire_exe = File::create(bin_path.join(filename))?;
                info!("Assembling {}", filename.display());

                let mut pre_compiled = File::open(entry.path())
                    .with_context(|| format!("Failed to open {}", entry.path().display()))?;

                std::io::copy(&mut pre_compiled, &mut acquire_exe)
                    .context("Failed to copy data from pre-compiled exe")?;
                std::io::copy(&mut deps, &mut acquire_exe)
                    .context("Failed to write dependencies to pre-compiled binaries")?;

                deps.seek(std::io::SeekFrom::Start(0))?;
            }
        }
    }

    info!("Happy hunting!");

    Ok(())
}

fn init_logger(log_level: LevelFilter) {
    env_logger::Builder::new()
        .format(|buf, record| {
            let ts = buf.timestamp();
            let level = record.level();
            let module = record.module_path().unwrap_or_default();
            let msg = record.args().to_string();
            // Nice colours
            let s = buf.default_level_style(level);

            // Handle multi-line messages (indent all but the first line)
            let mut lines = msg.lines();
            if let Some(first) = lines.next() {
                writeln!(buf, "[{} {s}{}{s:#} {}] {}", ts, level, module, first)?;
            }
            for line in lines {
                writeln!(buf, "[{} {s}{}{s:#} {}]    {}", ts, level, module, line)?; // 4-space indent for extra lines
            }
            Ok(())
        })
        .filter_level(log_level)
        .init();
}

fn create_bin_path(build_path: &Path) -> Result<PathBuf> {
    let bin_path = build_path.join("bin");
    match create_dir(bin_path.as_path()) {
        Ok(_) => (),
        Err(e) => match e.kind() {
            std::io::ErrorKind::AlreadyExists => (),
            _ => bail!("{}", e),
        },
    }

    Ok(bin_path)
}

fn open_deps_zip(build_path: &Path) -> Result<File> {
    let path = build_path.join("lib/lib.zip");
    let f = File::options()
        .read(true)
        .open(path.as_path())
        .with_context(|| format!("Failed to find {}", path.display()))?;

    Ok(f)
}
