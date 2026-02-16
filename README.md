# Acquire Builder 

Build standalone [acquire](https://github.com/fox-it/acquire) binaries for Windows, Linux and Mac. These binaries are built using the [python-build-standalone](https://github.com/astral-sh/python-build-standalone) distributions in combination with Rust. Using [pyo3](https://github.com/PyO3/pyo3), I managed to statically link python into a Rust binary, which is basically the same thing that [PyOxidizer](https://github.com/indygreg/PyOxidizer) does. Why use this project instead of just using PyOxidizer?
 - Support for Python 3.11
 - Support for acquire's output encryption
 - Static ucrt/vcrt
 - Specifically made for acquire.

I did not create a fancy in-memory module loader like PyOxidizer has. The necessary python resources are loaded from a zip file that is part of the binary. This way, no additional files are dropped to disk when running the python interpreter and loading resources.

## Quick start

> [!IMPORTANT]  
> Make sure that Python is installed on the system that you are running this builder on. If you are using the `compile` option, you need the Rust compiler installed as well.

```
# Download binaries from repository releases
$ ./acquire-builder download

# Compile standalone binary from scratch
$ ./acquire-builder compile
```

## Usage

```
Usage: acquire-builder [OPTIONS] <COMMAND>

Commands:
  compile   Compile new standalone binaries
  download  Download standalone binaries from GitHub release
  help      Print this message or the help of the given subcommand(s)

Options:
  -c, --config-file <CONFIG_FILE>
          Path to your custom acquire config file
  -o, --output-dir <OUTPUT_DIR>
          Path to store output files [default: build]
      --acquire-version <ACQUIRE_VERSION>
          Acquire version to use. Will use the latest version as default
      --dissect-version <DISSECT_VERSION>
          Dissect version to use. Will use the latest version as default
  -p, --python-exe <PYTHON_EXE>
          Path to local python executable
  -v, --verbose...
          More output per occurrence
  -q, --quiet...
          Less output per occurrence
  -h, --help
          Print help
```

There are two ways to use this project. Both result in a standalone acquire executable.
  1. Run in compile mode. This will compile the project from scratch. Might be nice if you want to make some changes to the binary yourself. No cross-compilation supported. 
  2. Run in download mode. This repository will host releases that contain pre-compiled executables so you don't have to compile these yourself. This is the fastest and easiest option. Will generate a binary for all supported platforms.

For both modes, a local python interpreter is needed. By default, the builder will use whatever interpreter lives behined the `python3` command, but you can specify a full path to a python interpreter using the `--python-exe` flag. You can also specify which acquire version you want to use and which dissect version. Only supply a version number, like `3.21` or even `3.22.dev3` if you want to use a development version.

You can add an acquire `config.py` file with the `--config-file` flag, which should look something like this:

```
PUBKEY = """
-----BEGIN RSA PUBLIC KEY-----
MIIBCgKCAQEA3uiQuKBjLmeV+Ey3G9lduYUbD5m/l63XACxtUOoHWkZko4UQCQu0
Z22Cp2GN3KGIVGHegTw4OicTC3Pdje5KOlIIZSe2YCMbVp1vr3kzWGpWFQemw+j0
B+I1QbH1Zv0FVjrY42mPBqgGqNnre10I44npvX8pFVuUvh2JCIcjKaWQ7LifQLZa
LsD74J+CQoBe5lW7vrTDWmPMJLRn9JlAh80xZUEG2XL8BF4ShVcLaAP+rk2ymNJD
GLtgfN67KpF8L7ziyLOb3yUdBdLA8e6k9r37QS6F6cV4NTFWVsciaUaqHax2olzn
YCfc5bW9lqm3+5XI0f8WO1BvVPg7VvdSjwIDAQAB
-----END RSA PUBLIC KEY-----
"""

CONFIG = {
    "arguments": ["--profile", "full", "--auto-upload"],
    "public_key": PUBKEY,
    "upload": {
        "mode": "cloud",
        "endpoint": "storage.googleapis.com",
        "access_id": "mCQgJ5N8fEBqUzR3t4iP",
        "access_key": "rbsoiJKM3vDnmZfBV4X7FaqQgezkHWN0PCytlOwT",
        "bucket": "asdf",
        "folder": "asdf",
    }
}

```

You will need to create this file yourself. The builder will not do this for you. All it does is embed the file in the standalone executable.

You can generate a keypair using openssl:

```
openssl genrsa -traditional -out private.key 2048
openssl rsa -traditional -outform PEM -in private.key -RSAPublicKey_out -out public.key
```

After running the builder, your binaries will be ready in the `bin` folder!

## To do

- Add support for Python 3.14
- Add support for (local) dev versions of acquire/dissect.target
- Add pystandalone functions for:
    * dissect/hypervisor/descriptor/vmx.py
    * dissect/apfs/objects/keybag.py
    * dissect/target/loaders/itunes.py

## Notes

For Linux, we only compile musl distributions. If you want to compile from scratch, make sure you compile from a Linux musl distribution, like Alpine Linux. I recommend the Rust Alpine docker image.

I did a test run on a couple VMs:
- Windows Server 2012 R2
- Windows 10
- ESXi 7
- Fedora 39
- MacOS 26

I expect no issues with modern/mainstream Linux distros and Windows versions from Server 2012 R2 and up. I do not own an Apple Silicon Mac, so I don't know how well it works on there. It does seem to run fine on the Github runner. If you run into a problem and you are sure that this is not a problem with acquire itself, feel free to open an issue!

## Credits

Took some code/inspiration from:
* https://github.com/indygreg/PyOxidizer
* https://github.com/littledivy/aead-gcm-stream
* https://github.com/Legrandin/pycryptodome
* All the references to `_pystandalone` in the acquire/dissect codebase

