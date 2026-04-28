use std::collections::HashMap;
use std::fmt::Write;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use dunce::canonicalize;
use reqwest::blocking::ClientBuilder;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tar::Archive;

use crate::platform::{Platform, get_platform};

pub static DISTS: LazyLock<HashMap<&str, &str>> = LazyLock::new(|| {
    HashMap::from([
        (
            "linux",
            "https://github.com/astral-sh/python-build-standalone/releases/download/20240224/cpython-3.11.8+20240224-x86_64-unknown-linux-musl-noopt-full.tar.zst",
        ),
        (
            "windows",
            "https://github.com/astral-sh/python-build-standalone/releases/download/20240224/cpython-3.11.8+20240224-x86_64-pc-windows-msvc-static-noopt-full.tar.zst",
        ),
        (
            "mac_intel",
            "https://github.com/astral-sh/python-build-standalone/releases/download/20240224/cpython-3.11.8+20240224-x86_64-apple-darwin-pgo-full.tar.zst",
        ),
        (
            "mac_arm",
            "https://github.com/astral-sh/python-build-standalone/releases/download/20240224/cpython-3.11.8+20240224-aarch64-apple-darwin-pgo-full.tar.zst",
        ),
    ])
});

// The necessary structs to parse the PYTHON.json
// file with serde.
#[derive(Deserialize)]
pub struct DistDescription {
    #[serde(skip)]
    local_path: PathBuf,

    python_implementation_name: String,
    python_major_minor_version: String,
    libpython_link_mode: String,
    python_exe: PathBuf,
    build_info: BuildInfo,
    python_paths: PythonPath,
}

#[derive(Deserialize)]
struct PythonPath {
    stdlib: String,
}

#[derive(Deserialize)]
struct BuildInfo {
    core: BuildInfoCore,
    extensions: HashMap<String, Vec<Extension>>,
}

#[derive(Deserialize)]
struct BuildInfoCore {
    static_lib: PathBuf,
    links: Vec<Link>,
}

#[derive(Deserialize)]
struct Link {
    name: String,
    system: Option<bool>,
    framework: Option<bool>,
}

#[derive(Deserialize)]
struct Extension {
    links: Vec<ExtensionLink>,
}

#[derive(Deserialize)]
struct ExtensionLink {
    name: String,
    path_static: Option<PathBuf>,
    system: Option<bool>,
    framework: Option<bool>,
}

impl DistDescription {
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let parent = path
            .as_ref()
            .parent()
            .ok_or_else(|| anyhow!("Failed to resolve distribution path"))?;

        let f = File::open(path.as_ref())
            .with_context(|| format!("Failed to open {}", path.as_ref().display()))?;
        let mut dist_description: DistDescription = serde_json::from_reader(f)?;
        dist_description.local_path = parent.to_path_buf().canonicalize()?;

        Ok(dist_description)
    }

    pub fn get_stdlib(&self) -> PathBuf {
        self.local_path.join(self.python_paths.stdlib.as_str())
    }

    pub fn get_python_exe(&self) -> PathBuf {
        self.local_path.join(self.python_exe.as_path())
    }

    pub fn get_pyo3_config(&self, dist_path: &Path) -> Result<String> {
        let mut config = String::new();
        let dist = get_platform();

        let implementation = {
            let implementation = self.python_implementation_name.to_lowercase();
            if implementation == "cpython" {
                String::from("CPython")
            } else {
                implementation.to_string()
            }
        };

        writeln!(config, "implementation={}", implementation)?;

        writeln!(config, "version={}", self.python_major_minor_version)?;

        if self.libpython_link_mode == "static" {
            writeln!(config, "shared=false")?;
        } else {
            writeln!(config, "shared=true")?;
        }

        writeln!(config, "abi3=false")?;

        let python_exe: PathBuf = self.get_python_exe();

        writeln!(config, "executable={}", python_exe.display())?;
        writeln!(config, "pointer_width=64")?;
        writeln!(config, "build_flags=")?;

        if let Platform::Windows = dist {
            writeln!(config, "suppress_build_script_link_lines=true")?
        } else {
            writeln!(config, "suppress_build_script_link_lines=false")?
        }

        // We need the name of the library and the path where we can find it.
        let libpython = parse_lib(dist_path, self.build_info.core.static_lib.as_path())?;

        let mut libname = libpython.0.clone();

        let link_lib = "extra_build_script_line=cargo:rustc-link-lib=";
        let link_lib_static = "extra_build_script_line=cargo:rustc-link-lib=static=";
        let link_lib_framework = "extra_build_script_line=cargo:rustc-link-lib=framework=";
        let link_search_native = "extra_build_script_line=cargo:rustc-link-search=native=";

        if !matches!(dist, Platform::Windows) {
            libname = libname
                .strip_prefix("lib")
                .context("Failed to parse libname")?
                .to_string()
        }

        let build_line_libpython = if let Platform::Windows = dist {
            format!("{}pythonXY:", link_lib_static)
        } else {
            link_lib_static.into()
        };

        writeln!(config, "{}{}", build_line_libpython, libname)?;
        writeln!(config, "{}{}", link_search_native, libpython.1.display())?;

        let links = self.build_info.core.links.as_slice();
        let extensions = &self.build_info.extensions;

        // Parse the core links.
        for lib in links {
            if let Some(system) = lib.system
                && system
            {
                writeln!(config, "{}{}", link_lib, lib.name)?;
            }

            if let Some(framework) = lib.framework
                && framework
            {
                writeln!(config, "{}{}", link_lib_framework, lib.name)?;
            }
        }

        // Parse the extension links.
        for attrs in extensions.values() {
            for attr in attrs {
                for link in &attr.links {
                    if let Some(path_static) = link.path_static.as_deref() {
                        let (_, search_path) = parse_lib(dist_path, path_static)?;
                        let search_path_config: String =
                            format!("{}{}", link_search_native, search_path.display());

                        if !config.contains(&search_path_config) {
                            writeln!(config, "{}", search_path_config)?;
                        }

                        let static_lib = format!("{}{}", link_lib_static, link.name);

                        if !config.contains(static_lib.as_str()) {
                            writeln!(config, "{}", static_lib)?;
                        }
                    }

                    if let Some(system) = link.system
                        && system
                    {
                        let config_string = format!("{}{}", link_lib, link.name);

                        if config.contains(&config_string) {
                            continue;
                        }

                        writeln!(config, "{}", config_string)?;
                    }

                    if let Some(framework) = link.framework
                        && framework
                    {
                        let config_string = format!("{}{}", link_lib_framework, link.name);

                        if config.contains(&config_string) {
                            continue;
                        }

                        writeln!(config, "{}", config_string)?;
                    }
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            let clang_path = get_clang_search_paths()?;
            writeln!(
                config,
                "extra_build_script_line=cargo:rustc-link-search={}",
                clang_path
            )?;
            writeln!(config, "{}clang_rt.osx", link_lib)?;
        }

        Ok(config)
    }
}

fn parse_lib<P: AsRef<Path>>(dist_path: P, lib: P) -> Result<(String, PathBuf)> {
    let dist_path = dist_path.as_ref().join("python");
    let err = || anyhow!("Failed to parse libpython path");

    let path = canonicalize(dist_path.join(lib.as_ref()))
        .with_context(|| format!("Failed to find {}", dist_path.join(lib.as_ref()).display()))?;
    let path = path.as_path();

    let parent = path.parent().ok_or_else(err)?.to_owned();
    let lib = path.file_stem().ok_or_else(err)?.to_owned();
    let lib = lib.display().to_string();

    Ok((lib, parent))
}

pub fn unarchive<P: AsRef<Path>>(filename: P) -> Result<PathBuf> {
    let source = File::open(filename.as_ref())
        .with_context(|| format!("Failed to open {}", filename.as_ref().display()))?;
    let dest = filename
        .as_ref()
        .parent()
        .ok_or_else(|| anyhow!("Failed to get unarchive dest dir."))?;

    let decoder = zstd::stream::Decoder::new(source)?;
    let mut archive = Archive::new(decoder);
    archive.unpack(dest).with_context(|| {
        format!(
            "Failed to unarchive python-build-standalone distribution to {}",
            dest.display()
        )
    })?;

    Ok(dest.to_path_buf())
}

pub fn download_dist<P: AsRef<Path>>(url: &str, path: P) -> Result<PathBuf> {
    let file_name = {
        let splitted: Vec<&str> = url.rsplitn(2, "/").collect();
        let name = splitted
            .first()
            .ok_or_else(|| anyhow!("Failed to parse file name"))?;

        name.to_owned()
    };

    let client = ClientBuilder::new()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(30))
        .build()?;

    let dist = client.get(url).send()?;
    let dist = dist.bytes()?.to_vec();

    let hash = client.get(String::from(url) + ".sha256").send()?.text()?;
    let calculated_hash = hex::encode(Sha256::digest(&dist));

    if calculated_hash != hash.trim() {
        log::error!("Original hash: {}", hash);
        log::error!("Calculated hash: {}", calculated_hash);
        bail!("Hash mismatch");
    }

    let file_path = path.as_ref().join(file_name);
    std::fs::write(&file_path, dist)?;

    Ok(file_path)
}

// We need the clang search paths on macos.
#[cfg(target_os = "macos")]
pub fn get_clang_search_paths() -> Result<String> {
    let output = std::process::Command::new("clang")
        .arg("--print-search-dirs")
        .output()?;

    if !output.status.success() {
        bail!("Failed to resolve clang search dirs")
    }

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.contains("libraries: =") {
            let path = line
                .split('=')
                .next_back()
                .ok_or_else(|| anyhow!("could not parse libraries line"))?;

            if PathBuf::from(path).exists() {
                return Ok(path.into());
            }
        }
    }

    Err(anyhow!("Failed to resolve clang search dirs."))
}
