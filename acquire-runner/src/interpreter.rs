use std::ffi::OsString;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::{
    ffi::{CString, NulError},
    os::unix::ffi::OsStrExt,
};

use anyhow::{Result, anyhow, bail};
use pyo3::append_to_inittab;
use pyo3::ffi::{
    Py_InitializeFromConfig, Py_RunMain, PyConfig, PyConfig_InitPythonConfig, PyStatus_Exception,
};

use crate::pystandalone::_pystandalone;

#[cfg(windows)]
#[allow(non_camel_case_types)]
type wchar_t = u16;

pub struct Interpreter(PyConfig);

impl Interpreter {
    pub fn init() -> Result<Self> {
        let mut config: PyConfig = unsafe { std::mem::zeroed() };
        unsafe { PyConfig_InitPythonConfig(&mut config) };

        // We will import from a ZIP so do not write any bytecode.
        config.write_bytecode = 0;
        // Disable sys.path warnings.
        config.pathconfig_warnings = 0;
        // Enable isolated mode because we do not want to be influenced by
        // local python setups.
        config.isolated = 1;
        // Do not parse argv for the python interpreter.
        config.parse_argv = 0;
        // Do not resolve search paths as we will set these manually.
        config.module_search_paths_set = 1;

        let args: Vec<OsString> = std::env::args_os().collect();
        set_argv(&mut config, args.as_slice())?;

        let current_exe = if let Ok(exe) = std::env::current_exe() {
            exe
        } else if let Ok(dir) = std::env::current_dir() {
            let exe = PathBuf::from(args[0].clone());
            dir.join(exe)
        } else {
            bail!("Failed to get the location of the current executable")
        };

        // let current_exe = current_exe.as_path().join("lib");
        let module_search_paths = vec![current_exe.as_path()];

        for path in module_search_paths {
            set_search_path(&mut config.module_search_paths, path)?;
        }

        set_config(&config, &config.run_module, "acquire.acquire\0")?;

        // Add _pystandalone extension
        append_to_inittab!(_pystandalone);

        Ok(Interpreter(config))
    }

    pub fn run(&self) -> Result<()> {
        unsafe {
            let status = Py_InitializeFromConfig(&self.0);

            if PyStatus_Exception(status) != 0 {
                eprintln!("Failed to initialize embedded python interpreter.");
            }

            Py_RunMain();
        }

        Ok(())
    }
}

impl Drop for Interpreter {
    fn drop(&mut self) {
        unsafe {
            if pyo3::ffi::Py_IsInitialized() != 0 {
                pyo3::ffi::PyGILState_Ensure();
                pyo3::ffi::Py_FinalizeEx();
            }
        }
    }
}

#[cfg(windows)]
fn set_config(config: &PyConfig, attribute: &*mut u16, value: &str) -> Result<()> {
    unsafe {
        let status = pyo3::ffi::PyConfig_SetBytesString(
            config as *const PyConfig as *mut PyConfig,
            attribute as *const *mut u16 as *mut *mut u16,
            value.as_ptr() as *const i8,
        );

        if PyStatus_Exception(status) != 0 {
            bail!("Python config change failed")
        } else {
            Ok(())
        }
    }
}

#[cfg(unix)]
fn set_config(config: &PyConfig, attribute: &*mut i32, value: &str) -> Result<()> {
    unsafe {
        let status = pyo3::ffi::PyConfig_SetBytesString(
            config as *const PyConfig as *mut PyConfig,
            attribute as *const *mut i32 as *mut *mut i32,
            value.as_ptr() as *const i8,
        );

        if PyStatus_Exception(status) != 0 {
            bail!("Python config change failed")
        } else {
            Ok(())
        }
    }
}

#[cfg(unix)]
pub fn set_argv(config: &mut PyConfig, args: &[OsString]) -> Result<()> {
    let argc = args.len() as isize;
    let argv = args
        .iter()
        .map(|x| CString::new(x.as_bytes()))
        .collect::<Result<Vec<_>, NulError>>()
        .map_err(|_| anyhow!("Failed to set argv."))?;
    let argvp = argv
        .iter()
        .map(|x| x.as_ptr() as *mut i8)
        .collect::<Vec<_>>();

    let status = unsafe {
        pyo3::ffi::PyConfig_SetBytesArgv(config as *mut _, argc, argvp.as_ptr() as *mut _)
    };

    if unsafe { pyo3::ffi::PyStatus_Exception(status) } != 0 {
        bail!("Failed to set argv")
    } else {
        Ok(())
    }
}

#[cfg(windows)]
pub fn set_argv(config: &mut PyConfig, args: &[OsString]) -> Result<()> {
    let argc = args.len() as isize;
    let argv = args
        .iter()
        .map(|x| {
            let mut buffer = x.encode_wide().collect::<Vec<u16>>();
            buffer.push(0);

            buffer
        })
        .collect::<Vec<_>>();
    let argvp = argv
        .iter()
        .map(|x| x.as_ptr() as *mut u16)
        .collect::<Vec<_>>();

    unsafe {
        let status = pyo3::ffi::PyConfig_SetArgv(config as *mut _, argc, argvp.as_ptr() as *mut _);

        if pyo3::ffi::PyStatus_Exception(status) != 0 {
            bail!("Failed to set argv")
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
fn set_search_path(dest: &mut pyo3::ffi::PyWideStringList, path: &Path) -> Result<()> {
    let mut value: Vec<wchar_t> = path.as_os_str().encode_wide().collect();
    // NULL terminate.
    value.push(0);

    let status =
        unsafe { pyo3::ffi::PyWideStringList_Append(dest as *mut _, value.as_ptr() as *const _) };

    if unsafe { pyo3::ffi::PyStatus_Exception(status) } != 0 {
        bail!("Failed to set search path")
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn set_search_path(dest: &mut pyo3::ffi::PyWideStringList, path: &Path) -> Result<()> {
    let value = path
        .as_os_str()
        .to_str()
        .ok_or_else(|| anyhow!("Failed to set search path"))?;

    let value = CString::new(value)?;

    let decoded = unsafe { pyo3::ffi::Py_DecodeLocale(value.as_ptr(), std::ptr::null_mut()) };

    let status = unsafe { pyo3::ffi::PyWideStringList_Append(dest as *mut _, decoded) };

    if unsafe { pyo3::ffi::PyStatus_Exception(status) } != 0 {
        bail!("Failed to set search path")
    } else {
        Ok(())
    }
}
