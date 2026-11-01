# gaP — great! another Portabilizer

[![Release](https://img.shields.io/github/v/release/Hanatarou/gaP)](https://github.com/Hanatarou/gaP/releases/latest)
[![Build](https://img.shields.io/github/actions/workflow/status/Hanatarou/gaP/release.yml)](https://github.com/Hanatarou/gaP/actions)
[![Downloads (all releases)](https://img.shields.io/github/downloads/Hanatarou/gaP/total?label=downloads%20%28all%29)](https://github.com/Hanatarou/gaP/releases)
[![Downloads (latest release)](https://img.shields.io/github/downloads/Hanatarou/gaP/latest/total?label=downloads%20%28latest%29)](https://github.com/Hanatarou/gaP/releases/latest)
[![License: MIT](https://img.shields.io/github/license/Hanatarou/gaP)](LICENSE)
![Platform](https://img.shields.io/badge/platform-Windows%20x64%20%7C%20x86-blue)
![Language](https://img.shields.io/github/languages/top/Hanatarou/gaP)

## 🤖 A Note on AI Usage

I built this project alone, as a personal project, with substantial help from AI tools — mainly Claude. I believe knowledge only survives past us if it's shared, and that's the spirit behind releasing this for free.

I did this on my own time and dime, covering all costs myself, without asking anyone for donations.

Just as I respect opinions against the use of AI, I expect the use of AI here — as a tool that helped me build this project — to be respected in return. Disrespect toward this work, toward me, or toward anyone else involved will not be tolerated.

All AI-generated content is reviewed and validated by me before being committed — I stand behind every decision to include code in this repository, regardless of how it was originally written. This project is still provided as-is, under the MIT license below, with no warranty of any kind.

If you're uncomfortable with AI-assisted code for any reason, you are under no obligation to use, contribute to, or engage with this project. No hard feelings — just move on.

For everyone else: bug reports and PRs are evaluated on their merits (does it work, is it correct), not on how the code was produced.

## 🙏 Credits: yaP

gaP is based on **yaP** by **tproli** (Roland Toth). I (Bruno Eduardo, known as **Aeolis** on The Portable Freeware Collection forums) created gaP in AutoIt in 2015; this repository is the **Rust port** of it: the same ini format, the same keywords, the same macros and the same text encryption, so a launcher configuration written for the AutoIt gaP keeps working. The last AutoIt version was 1.1.5.0; the Rust port starts at 2.0.0.0. My thanks go to everyone at The Portable Freeware Collection forums who tested and discussed it.

- **gaP forum topic:** http://www.portablefreeware.com/forums/viewtopic.php?f=11&t=22129
- **yaP:** http://rolandtoth.hu/yaP/

## 🎯 What is gaP?

gaP is a universal DIY (Do It Yourself) launcher for making **portable applications on Windows**. You tell it in a plain `.ini` file which files, folders, registry keys and values the application uses; gaP copies them next to the launcher, starts the application, waits until it closes, then saves the settings back to the portable folder and restores what was on the computer before.

- ✅ One small executable, no installer, no runtime to install (the C runtime is linked statically)
- ✅ Settings in a plain `.ini` next to the launcher: `MyAppPortable.exe` reads `MyAppPortable.ini`
- ✅ 64-bit and 32-bit builds that read the same `.ini`; the application that is launched is chosen by the architecture of Windows (`application = App_x86.exe :: App_x64.exe`)
- ✅ Does not need administrator rights unless the portabilized application does (`!admin`)

What it can do:

- portabilize an application by listing directories, files, registry keys and registry values;
- run the 64-bit version of the main application automatically when the OS is 64-bit and one is set;
- create, copy, move and delete files, directories, registry keys and values;
- handle x86 and x64 registry entries with the appropriate switch in the registry keywords;
- check administrator rights, multiple instances, OS version, .NET, Java, Internet connection and read-only media;
- ask for administrator rights when they are required;
- execute batch files, import and export registry keys, merge `.reg` files;
- read and write `.ini` files, replace strings or lines in text files, set environment variables;
- show custom messages, put text on the clipboard, run applications, kill processes;
- create, delete, start and stop services, register and unregister DLLs, create symbolic links;
- delete the oldest or newest files or folders by date or name (to keep a number of backups);
- expand environment variables, shell folders and gaP macros (paths, dates, text, language);
- set file and directory attributes, refresh the shell icons;
- **encrypt configuration items** with the `-crypt` parameter;
- clean the `MuiCache` and `App Paths` values from the registry.

## 📦 Installation

There is no installer. Download the zip of your architecture from the [releases](https://github.com/Hanatarou/gaP/releases/latest), extract it and use `gaP.exe`. The zip holds only the executable. Each zip comes with a `.exe.sha256.txt` with the SHA-256 of the executable (not of the zip), so you can verify it after extracting.

| Download | For |
|---|---|
| `gaP-v2.0.0.0-windows-x64.zip` | 64-bit Windows |
| `gaP-v2.0.0.0-windows-x86.zip` | 32-bit and 64-bit Windows |

(`v2.0.0.0` is the version; each zip has its `gaP-v2.0.0.0-windows-x64.exe.sha256.txt` beside it, outside the zip.) Both builds do the same thing and read the same `.ini`.

### x86 and x64

The same `.ini` works with both builds. What decides the architecture is the **application**, not the build of gaP:

- the first segment of `application` is the x86 application and the second one is the x64 application; use `NULL` as the first segment for an application that exists only as x64;
- the `System32` folder in paths, `regsvr32` and `cmd.exe` (`regdll`, `batch`) and the Java/.NET checks (`!java`, `!dotnet`) follow the application that was launched;
- the registry flag (`x86` or `x64`) is always explicit and defaults to `x86`, whatever the build is; and
- to change from one build to the other, replace `gaP.exe` and keep its name: the `.ini` has the same name.

### Requirements
- Windows 10 or later (a limit of the Rust standard library, not of gaP itself). The C runtime is linked statically: nothing to install.

## 🚀 Usage

1. Create a folder for the application ("MyApp", for instance) and put its files in a subfolder ("MyApp\App"). Use an extractor to get the files if it is an installer.
2. Copy `gaP.exe` to the "MyApp" folder and **rename it** ("MyAppPortable.exe", for instance). The name matters: the settings file has the same name.
3. Run "MyAppPortable.exe" and click Ok to generate a new settings file ("MyAppPortable.ini").
4. Edit the `.ini`: set the path of the application (`application = App\MyApp.exe`) and list the files, directories, registry keys or values to portabilize.
5. Run "MyAppPortable.exe".

A minimal `.ini`:

```ini
[GENERAL]
application = App\MyApp_x86.exe :: App\MyApp_x64.exe
commandline = /nosplash
!multiple = -1

[BEFORE]
dir = %Appdata%\MyApp :: Data\MyApp\
regkey = HKCU\Software\MyApp :: Data\settings.reg

[AFTER]
```

An application that exists only as 64-bit, and a registry key of the 64-bit view:

```ini
[GENERAL]
application = NULL :: App\MyApp_x64.exe

[BEFORE]
regkey = HKLM\Software\MyApp :: Data\settings.reg :: x64
```

`dir`, `file`, `regkey` and `regvalue` are "wrap" items: they run before and after the application. The complete reference (every keyword, with syntax and examples) is in `gaP_Help.txt`; run `gaP.exe -help` to create it next to the program.

### Parameters

| Parameter | What it does |
|---|---|
| `-crypt` | Opens the interface to encrypt and decrypt text for the `.ini` |
| `-license` | Shows the license (MIT) |
| `-about` | Shows the about window |
| `-help` | Creates a copy of the help file next to `gaP.exe` |

### Keywords

| Section | Keywords |
|---|---|
| `GENERAL` | `application` `commandline` `workdir` `!admin` `!dotnet` `!java` `!multiple` `!online` `!os` `!readonly` `!waitprocess` |
| Wrap (before and after) | `dir` `file` `regkey` `regvalue` |
| Backup | `(dir)` `(file)` `(regkey)` `(regvalue)` |
| Copy / move (`<-` to reverse) | `->dir` `->file` `->regkey` `->regvalue` |
| Create | `+dir` `+file` `+regkey` `+regvalue` |
| Delete | `-dir` `-file` `-regkey` `-regvalue` |
| Misc | `attributes` `batch` `clean` `clip` `date` `delay` `envvar` `iniread` `iniwrite` `keeponly` `killprocess` `message` `nowait` `quit` `regdll` `regexport` `regimport` `regmerge` `replace` `replaceline` `restore` `run` `service` `stringreplace` `symlink` `systemrefresh` `uservar` |

Any keyword followed by `x86` or `x64` (`keywordx64 = ...`) is processed only if the architecture of Windows matches.

### Macros

| Kind | Macros |
|---|---|
| Path | `{GAPROOT}` `{EXEPATH}` `{APPEXE}` `{DRIVE}` `{USERFILE}` `{USERDIR}` `{WORKDIR}` `{JAVAEXE}` |
| Text | `{CLIPBOARD}` `{LINEBREAK}` `{USERTEXT}` |
| Date and time | `{d}` `{j}` `{m}` `{n}` `{Y}` `{y}` `{g}` `{G}` `{h}` `{H}` `{i}` `{s}` `{U}` |
| Language | `{LANGCODE}` `{LANG}` `{LOCALE}` `{LOCALENAME}` `{LOCALENAME_SHORT}` `{COUNTRY}` |
| Misc | `{OSARCH}` `{COMMANDLINE}` `{PARAMETERS}` and your own variables (`{MyVar}`) |

To write a real curly bracket, escape it with `<`: `<{` and `<}`.

## 🔐 Encrypted text

Run `gaP.exe -crypt` to encrypt a text (a password, a path) and paste the result in the `.ini`. When gaP finds an encrypted text in the configuration it asks for the password.

The format is the one of the AutoIt gaP (AES-256, CBC, the key derived from the password with MD5 as Windows CryptoAPI's `CryptDeriveKey` does, zero IV, PKCS#7 padding, hexadecimal output between `!@@!`), so texts encrypted by the original program can still be decrypted. This keeps a text out of plain sight; it is not a modern password-based encryption (no salt, no slow key derivation), so do not use it to protect something that needs real protection.

## 🔍 What it does *not* do

- ❌ No splash screen.
- ❌ It is not a virtualization layer: it copies files and registry entries and puts them back, so a crash in the middle of a session can leave data on the computer.
- ❌ It is not designed to be compatible with the PortableApps.com format.

## 🛠️ Build

Rust, with the MSVC toolchain (Visual Studio or the Build Tools with the C++ workload; the icon, the version information and the manifest are compiled by the resource compiler that comes with them). The C runtime is linked statically through `.cargo/config.toml`.

```
cargo build --release
-> target\release\gap.exe
```

`cargo build --release` builds for the architecture of your toolchain. To choose one, install the target once and pass it:

```
rustup target add x86_64-pc-windows-msvc i686-pc-windows-msvc
```

**The released programs** are the smallest ones: profile `release` (size-optimised, LTO, `panic = "abort"`, stripped) built with a nightly Rust that rebuilds the standard library for size and drops the panic machinery (`-Cpanic=immediate-abort`). `.github/workflows/release.yml` builds the 64-bit and the 32-bit program, runs `clippy` and the tests for both, and attaches the zips and their `.exe.sha256.txt` to the release; it refuses a release whose tag is not the version of `Cargo.toml` and `res/resources.rc` (`v2.0.0.0`). The nightly is not pinned: every build uses the newest one. To build them yourself, install the nightly once:

```
rustup toolchain install nightly -c rust-src
```

Then, in a Developer Command Prompt (or any prompt where `cargo` finds the Visual Studio Build Tools):

```
set RUSTFLAGS=-Zunstable-options -Cpanic=immediate-abort -C target-feature=+crt-static
```

(PowerShell: `$env:RUSTFLAGS = "-Zunstable-options -Cpanic=immediate-abort -C target-feature=+crt-static"`.) `RUSTFLAGS` makes Cargo ignore `.cargo/config.toml`, which is where `+crt-static` is, so it is repeated here; without it the program would need the Visual C++ Redistributable.

64-bit:

```
cargo +nightly build --release --target x86_64-pc-windows-msvc ^
      -Z build-std=std,panic_abort -Z build-std-features=optimize_for_size
-> target\x86_64-pc-windows-msvc\release\gap.exe
```

32-bit:

```
cargo +nightly build --release --target i686-pc-windows-msvc ^
      -Z build-std=std,panic_abort -Z build-std-features=optimize_for_size
-> target\i686-pc-windows-msvc\release\gap.exe
```

Rename the result to `gaP.exe` (or to the name of your launcher).

### Tests

```
cargo test
```

The unit tests cover, among other things, the text encryption (round trip and the SHA-1 test vector), the macro parser (the `{VAR}` escapes and the detection of an undefined `%VAR%`), the building of registry values, the values of `!os` and the rewriting of `System32` paths for the architecture of the application. They do not replace trying a launcher on a real application, nor running the same `.ini` with the x86 and the x64 build on a 64-bit Windows.

### Dependencies

[`windows-sys`](https://crates.io/crates/windows-sys) for the Win32 API, the [RustCrypto](https://github.com/RustCrypto) crates `aes`, `cbc`, `cipher`, `md-5` and `sha1` for the text encryption and SHA-1, and [`embed-resource`](https://crates.io/crates/embed-resource) (build only) for the icon, version information and manifest. There is no `Cargo.lock` in the repository on purpose: every build takes the newest versions of the crates that `Cargo.toml` allows.

## ⚠️ Warning

- gaP copies, moves and **deletes** files and registry keys as you tell it in the `.ini`. Try a new configuration on a copy first.
- Some programs restart themselves or update themselves and leave settings behind; update a portabilized application by hand (see the FAQ in the help file).
- Do not put a secret you care about in an `.ini`, even encrypted; see *Encrypted text*.

## 📚 Additional Info
- **Source code and releases:** https://github.com/Hanatarou/gaP
- **Original program:** gaP in AutoIt, by Bruno Eduardo, 2015 (last version 1.1.5.0)
- **Author:** Bruno Eduardo, https://github.com/Hanatarou
- **License:** MIT (see [LICENSE](LICENSE)); the same text is shown by `gaP.exe -license`
