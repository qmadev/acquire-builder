use std::fs::{File, create_dir};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use dunce::canonicalize;
use log::{Level, debug, error, info, log_enabled};
use reqwest::header;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::Deserialize;
use tar::Archive;
use walkdir::WalkDir;
use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::http::CLIENT;
use crate::platform::{Platform, get_platform};
use crate::python::distribution::{DISTS, DistDescription, download_dist, unarchive};

#[derive(Deserialize)]
struct DissectDeps {
    info: DissectDepsInfo,
}

#[derive(Deserialize)]
struct DissectDepsInfo {
    requires_dist: Vec<String>,
}

#[derive(Deserialize)]
struct GithubRelease {
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    url: String,
}

// We will use this struct to build the
// ZIP file containing everything python
// needs to run properly.
pub struct PythonEnvBuilder {
    build_path: PathBuf,
    dist_path: PathBuf,
    lib_path: PathBuf,
    config_file: Option<PathBuf>,
    acquire_version: Option<String>,
    dissect_version: Option<String>,
    local_python_exe: Option<PathBuf>,
}

impl PythonEnvBuilder {
    // Use this to create a new instance of the assembler
    // specifying the build_dir/output_dir.
    pub fn new(
        build_path: Option<PathBuf>,
        config_file: Option<PathBuf>,
        acquire_version: Option<String>,
        dissect_version: Option<String>,
        local_python_exe: Option<PathBuf>,
    ) -> Self {
        let build_path = build_path.unwrap_or_else(|| PathBuf::from("build"));

        PythonEnvBuilder {
            build_path: build_path.clone(),
            dist_path: build_path.join("dist"),
            lib_path: build_path.join("lib"),
            config_file,
            acquire_version,
            dissect_version,
            local_python_exe,
        }
    }

    pub fn get_build_path(&self) -> PathBuf {
        self.build_path.clone()
    }

    pub fn get_dist_path(&self) -> PathBuf {
        self.dist_path.clone()
    }

    pub fn assemble(&mut self, download_release: Option<String>) -> Result<()> {
        self.init_dir_structure()
            .context("Failed to initialize directory structure")?;

        info!("Downloading python-build-standalone distribution");
        self.download_dist()
            .context("Failed to download python-build-standalone distribution")?;

        let dist_description =
            DistDescription::from_path(self.dist_path.join("python/PYTHON.json"))?;
        let python_exe = dist_description.get_python_exe();
        let stdlib = dist_description.get_stdlib();

        if let Some(release) = download_release {
            self.download_release(release.as_str())
                .with_context(|| format!("Failed to download GitHub release {}", release))?;
        }

        info!("Generating Pyo3 config file");
        self.write_pyo3_config(&dist_description)
            .context("Failed to write pyo3 config file")?;

        info!("Downloading acquire dependencies");
        self.get_acquire_deps(&python_exe)
            .context("Failed to download acquire dependencies")?;

        info!("Generating _pluginlist.py");
        self.generate_pluginlist()
            .context("Failed to generate _pluginlist.py")?;

        info!("Building ZIP with all python dependencies");
        self.build_deps_zip(&stdlib)
            .context("Failed to build ZIP file with dependencies")?;

        Ok(())
    }

    pub fn assemble_for_target(&self, target: Platform) -> Result<()> {
        let target: String = target.into();
        let url = DISTS[target.as_str()];
        let download_path = self.dist_path.join(target);

        match create_dir(download_path.as_path()) {
            Ok(_) => debug!("Created dir: {}", self.build_path.display()),
            Err(e) => match e.kind() {
                std::io::ErrorKind::AlreadyExists => (),
                _ => bail!("{}", e),
            },
        }

        let dist_path = download_dist(url, download_path)?;
        let dist_path = unarchive(dist_path)?;
        let dist_description = DistDescription::from_path(dist_path.join("python/PYTHON.json"))?;
        let stdlib = dist_description.get_stdlib();
        self.build_deps_zip(stdlib.as_path())?;

        Ok(())
    }

    fn init_dir_structure(&mut self) -> Result<()> {
        match create_dir(self.build_path.as_path()) {
            Ok(_) => debug!("Created dir: {}", self.build_path.display()),
            Err(e) => match e.kind() {
                std::io::ErrorKind::AlreadyExists => (),
                _ => bail!("{}", e),
            },
        }

        match create_dir(self.dist_path.as_path()) {
            Ok(_) => debug!("Created dir: {}", self.dist_path.display()),
            Err(e) => match e.kind() {
                std::io::ErrorKind::AlreadyExists => (),
                _ => bail!("{}", e),
            },
        }

        match create_dir(self.lib_path.as_path()) {
            Ok(_) => debug!("Created dir: {}", self.lib_path.display()),
            Err(e) => match e.kind() {
                std::io::ErrorKind::AlreadyExists => (),
                _ => bail!("{}", e),
            },
        }

        let build_path = canonicalize(self.build_path.as_path())?;
        let dist_path = canonicalize(self.dist_path.as_path())?;
        let lib_path = canonicalize(self.lib_path.as_path())?;

        self.build_path = build_path;
        self.dist_path = dist_path;
        self.lib_path = lib_path;

        Ok(())
    }

    fn download_dist(&mut self) -> Result<()> {
        let dist: String = get_platform().into();
        let url = DISTS[dist.as_str()];
        let dist_file = download_dist(url, self.dist_path.as_path())?;
        unarchive(dist_file)?;

        Ok(())
    }

    fn download_release(&self, release: &str) -> Result<()> {
        let url = if release == "latest" {
            String::from("https://api.github.com/repos/qmadev/acquire-builder/releases/latest")
        } else {
            format!(
                "https://api.github.com/repos/qmadev/acquire-builder/releases/tags/{}",
                release
            )
        };

        let result = CLIENT.get(url.as_str(), None)?.text()?;

        let github_release: GithubRelease = serde_json::from_str(result.as_str())
            .context("Failed to parse Github API response for releases")?;

        let assets = github_release.assets;

        match create_dir(self.dist_path.join("pre-compiled")) {
            Ok(_) => (),
            Err(e) => match e.kind() {
                std::io::ErrorKind::AlreadyExists => (),
                _ => bail!("{}", e),
            },
        }

        let mut header = HeaderMap::new();
        header.insert(
            header::ACCEPT,
            HeaderValue::from_str("application/octet-stream")?,
        );

        for asset in assets {
            if asset.name != "pre-compiled.tar" {
                continue;
            }

            let download_url = asset.url;
            let filename = asset.name;

            debug!("Downloading {}", filename);

            let bytes = CLIENT.get(download_url.as_str(), Some(&header))?.bytes()?;
            let tar = Cursor::new(bytes);
            Archive::new(tar)
                .unpack(self.dist_path.join("pre-compiled"))
                .context("Failed to unpack pre-compiled.tar")?;
        }

        Ok(())
    }

    fn write_pyo3_config(&self, dist_description: &DistDescription) -> Result<()> {
        let pyo3_config = dist_description
            .get_pyo3_config(self.dist_path.as_path())
            .context("Failed to generate pyo3 build config")?;

        std::fs::write(self.build_path.join("pyo3-build-config.txt"), pyo3_config)
            .context("Failed to write pyo3-build-config.txt")?;

        Ok(())
    }

    fn get_acquire_deps(&self, python_exe: &Path) -> Result<()> {
        let debug = log_enabled!(Level::Debug);
        let base_args: Vec<&str> = vec![
            "-m",
            "pip",
            "download",
            "--only-binary=:all:",
            "--platform=manylinux2014_x86_64",
            "--dest",
        ];

        let mut args: Vec<String> = base_args.iter().map(|x| x.to_string()).collect();
        let dissect_deps = self.get_dissect_deps()?;
        let output_dir = self.lib_path.display().to_string();

        args.push(output_dir.clone());
        args.push(String::from("--no-deps"));
        args.extend_from_slice(dissect_deps.as_slice());

        debug!("Executing {} with args: {:?}", python_exe.display(), args);
        let execute = Command::new(python_exe)
            // If we write bytecode to disk, it will show up in our final zip
            // so let's not do that.
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .args(args)
            .stdout(if debug { Stdio::inherit() } else { Stdio::null() })
            .stderr(if debug { Stdio::inherit() } else { Stdio::null() })
            .output()?;

        if !execute.status.success() {
            error!("Failed to execute pip.")
        }

        let mut args: Vec<String> = base_args.iter().map(|x| x.to_string()).collect();

        args.push(output_dir.clone());
        args.push(format!("--find-links={}", output_dir));

        let acquire_download = if let Some(version) = self.acquire_version.clone() {
            format!("acquire=={}", version)
        } else {
            String::from("acquire")
        };

        args.push(acquire_download);
        args.push(String::from("minio==7.1"));

        debug!("Executing {} with args: {:?}", python_exe.display(), args);
        let execute = Command::new(python_exe)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .args(args)
            .stdout(if debug { Stdio::inherit() } else { Stdio::null() })
            .stderr(if debug { Stdio::inherit() } else { Stdio::null() })
            .output()?;

        if !execute.status.success() {
            error!("Failed to execute pip.")
        }

        Ok(())
    }

    // We need to merge all the wheels and the python stdlib
    // to a single zip file that we can use to import from.
    fn build_deps_zip(&self, stdlib: &Path) -> Result<()> {
        let wheels = std::fs::read_dir(self.lib_path.as_path())?;
        let file = File::options()
            .read(true)
            .write(true)
            .truncate(true)
            .create(true)
            .open(self.lib_path.join("lib.zip"))?;

        let mut zip = ZipWriter::new(file);

        for wheel in wheels.flatten() {
            if wheel.path().is_dir() {
                continue;
            }
            if wheel.path().ends_with("lib.zip") {
                continue;
            }
            if wheel.path().ends_with("_pluginlist.py") {
                continue;
            }

            let new_file = std::fs::File::open(wheel.path())?;
            let new_archive = ZipArchive::new(new_file)
                .with_context(|| format!("Failed to open {}", wheel.path().display()))?;

            zip.merge_archive(new_archive)
                .with_context(|| format!("Failed to merge {}", wheel.path().display()))?;
        }

        debug!("Adding {} to ZIP file", stdlib.display());
        zip_dir_recursive(&mut zip, stdlib)?;
        add_custom_files_to_zip(
            &mut zip,
            self.lib_path.join("_pluginlist.py"),
            self.config_file.clone(),
        )?;

        zip.finish()?;

        Ok(())
    }

    // To build the pluginlist, we need a fully working
    // version of dissect.target. To make sure that we have
    // the correct versions, we will use the downloaded version
    // and install that in a virtual environment.
    fn generate_pluginlist(&self) -> Result<()> {
        let debug = log_enabled!(Level::Debug);
        let zip_path = self.lib_path.clone();
        let lib_path_string = zip_path.display().to_string();

        let venv_path = format!("{}/dissect-target-venv", lib_path_string);

        let python_exe = if let Some(exe) = self.local_python_exe.as_ref() {
            exe
        } else {
            &PathBuf::from("python3")
        };

        let args = vec!["-m", "venv", venv_path.as_str()];

        debug!("Executing {} with args: {:?}", python_exe.display(), args);
        let create_venv = Command::new(python_exe)
            .args(args)
            .stdout(if debug { Stdio::inherit() } else { Stdio::null() })
            .stderr(if debug { Stdio::inherit() } else { Stdio::null() })
            .output()
            .with_context(|| {
                format!(
                    "Failed to execute {}. Are you sure it exists?",
                    python_exe.display()
                )
            })?;

        if !create_venv.status.success() {
            bail!(
                "Failed to create local venv!\n{:?}",
                String::from_utf8_lossy(create_venv.stderr.as_slice())
            );
        }

        let dist = get_platform();

        let command = if let Platform::Windows = dist {
            format!("{}/Scripts/pip.exe", &venv_path)
        } else {
            format!("{}/bin/pip", &venv_path)
        };

        let wheel = self.get_dissect_target_wheel()?.display().to_string();
        let args = vec!["install", wheel.as_str()];

        debug!("Executing {} with args {:?}", command, args);
        let install_dissect_target = Command::new(command)
            .args(args)
            .stdout(if debug { Stdio::inherit() } else { Stdio::null() })
            .stderr(if debug { Stdio::inherit() } else { Stdio::null() })
            .output()?;

        if !install_dissect_target.status.success() {
            bail!(
                "Failed to install dissect.target in local venv! {:?}",
                String::from_utf8_lossy(install_dissect_target.stderr.as_slice())
            );
        }

        let command = if let Platform::Windows = dist {
            format!("{}/Scripts/target-build-pluginlist.exe", &venv_path)
        } else {
            format!("{}/bin/target-build-pluginlist", &venv_path)
        };

        debug!("Executing {}", command);
        let generate_pluginlist = Command::new(command).output()?;

        if debug {
            let output = String::from_utf8_lossy(generate_pluginlist.stdout.as_slice());
            debug!("{}", output);
        }

        if !generate_pluginlist.status.success() {
            bail!(
                "Failed to generate pluginlist!\n{:?}",
                String::from_utf8_lossy(generate_pluginlist.stderr.as_slice())
            );
        }

        std::fs::write(
            self.lib_path.join("_pluginlist.py"),
            generate_pluginlist.stdout.as_slice(),
        )
        .context("Failed to write _pluginlist.py")?;

        Ok(())
    }

    fn get_dissect_target_wheel(&self) -> Result<PathBuf> {
        let wheels = std::fs::read_dir(self.lib_path.as_path())?;
        for wheel in wheels.flatten() {
            if wheel
                .file_name()
                .display()
                .to_string()
                .starts_with("dissect_target")
            {
                return Ok(wheel.path());
            }
        }

        Err(anyhow!("Failed to find local dissect.target wheel."))
    }

    fn get_dissect_deps(&self) -> Result<Vec<String>> {
        let pypi_url = if let Some(version) = self.dissect_version.clone() {
            format!("https://pypi.org/pypi/dissect/{}/json", version)
        } else {
            String::from("https://pypi.org/pypi/dissect/json")
        };

        let response = CLIENT.get(pypi_url.as_str(), None)?.text()?;

        let json_response: DissectDeps =
            serde_json::from_str(response.as_str()).context("Failed to parse json response")?;

        let deps = json_response.info.requires_dist;
        let mut dissect_deps: Vec<String> = Vec::new();

        // Do not need these to run acquire
        let unnecessary_deps = [
            "dissect.cim",
            "dissect.clfs",
            "dissect.etl",
            "dissect.executable",
            "dissect.ole",
            "dissect.shellitem",
            "dissect.thumbcache",
        ];

        'outer: for dep in deps {
            for redundant in unnecessary_deps {
                if dep.to_string().contains(redundant) {
                    continue 'outer;
                }
            }

            // We never want "full" packages
            if dep.to_string().contains("[full]") {
                let new = dep.to_string().replace("[full]", "");
                dissect_deps.push(new);
                continue;
            }

            if let Some(splitted) = dep.split_once(";") {
                dissect_deps.push(String::from(splitted.0));
                continue;
            }

            dissect_deps.push(dep.to_string());
        }

        dissect_deps = dissect_deps.iter().map(|x| x.replace('"', "")).collect();

        Ok(dissect_deps)
    }
}

// Some __init__.py files are not present in the wheels
// so we need to add them manually.
fn add_custom_files_to_zip(
    zip: &mut ZipWriter<File>,
    pluginlist: PathBuf,
    config_file: Option<PathBuf>,
) -> Result<()> {
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::default())
        .unix_permissions(0o755);

    zip.start_file("dissect/__init__.py", options)?;
    zip.write_all(b"")?;
    zip.start_file("flow/__init__.py", options)?;
    zip.write_all(b"")?;

    let pluginlist = std::fs::read(pluginlist)?;
    zip.start_file("dissect/target/plugins/_pluginlist.py", options)?;
    zip.write_all(pluginlist.as_slice())?;

    if let Some(config) = config_file {
        let acquire_config = std::fs::read(config)?;
        zip.start_file("acquire/config.py", options)?;
        zip.write_all(acquire_config.as_slice())?;
    }

    Ok(())
}

fn zip_dir_recursive(zip: &mut ZipWriter<File>, dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Err(ZipError::FileNotFound.into());
    }

    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::default())
        .unix_permissions(0o755);

    let walkdir = WalkDir::new(dir);
    let exclude = ["config-3.11", "test", "unittest"];

    for entry in walkdir.into_iter().flatten() {
        let path = entry.path();
        let name = path.strip_prefix(dir)?;
        let path_as_string = name
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("Failed to parse path"))?;

        if exclude.iter().any(|x| path_as_string.starts_with(x)) {
            continue;
        }

        if path.is_file() {
            zip.start_file(path_as_string, options)?;
            let mut f = File::open(path)?;
            std::io::copy(&mut f, zip)?;
        } else if !name.as_os_str().is_empty() {
            zip.add_directory(path_as_string, options)?;
        }
    }

    Ok(())
}
