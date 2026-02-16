#[derive(PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    MacIntel,
    MacArm,
}

impl Platform {
    pub fn target_triple(&self) -> String {
        match self {
            Platform::Windows => "x86_64-pc-windows-msvc".into(),
            Platform::Linux => "x86_64-unknown-linux-musl".into(),
            Platform::MacIntel => "x86_64-apple-darwin".into(),
            Platform::MacArm => "aarch64-apple-darwin".into(),
        }
    }

    pub fn from_filename<S: AsRef<str>>(s: S) -> Self {
        match s.as_ref() {
            "acquire-x86_64-pc-windows-msvc.exe" => Platform::Windows,
            "acquire-x86_64-unknown-linux-musl" => Platform::Linux,
            "acquire-x86_64-apple-darwin" => Platform::MacIntel,
            "acquire-aarch64-apple-darwin" => Platform::MacArm,
            _ => get_platform(),
        }
    }
}

impl From<Platform> for String {
    fn from(platform: Platform) -> String {
        match platform {
            Platform::Windows => "windows".into(),
            Platform::Linux => "linux".into(),
            Platform::MacIntel => "mac_intel".into(),
            Platform::MacArm => "mac_arm".into(),
        }
    }
}

pub fn get_platform() -> Platform {
    #[cfg(target_os = "windows")]
    return Platform::Windows;

    #[cfg(target_os = "linux")]
    return Platform::Linux;

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Platform::MacIntel;

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Platform::MacArm;
}
