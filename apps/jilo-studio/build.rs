//! Windows only: embed the app icon and version info (VERSIONINFO) into `jilo-studio.exe`, so it
//! shows in Explorer, the taskbar, the Start menu and Alt-Tab.
//!
//! On every other target this does nothing (`winresource` is only a build-dependency on Windows
//! hosts). With MSVC `winresource` runs `rc.exe`; with MinGW (`x86_64-pc-windows-gnu`) this script
//! runs `windres` itself, because `winresource` hands it the package folder unquoted and windres's
//! preprocessor then fails on a path with a space in it (`…\Jilo Studio\apps\jilo-studio`). A
//! missing resource compiler is a warning, unless `JILO_REQUIRE_WINRES=1` (release builds,
//! packaging/windows/package.ps1) makes it an error.

use std::path::{Path, PathBuf};

const PRODUCT: &str = "Jilo Studio";
const COMPANY: &str = "Rustam Gulov";
const COPYRIGHT: &str = "Copyright (c) Rustam Gulov and the Jilo Studio contributors. MIT OR Apache-2.0.";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/brand/jilo/jilo-studio.ico");
    println!("cargo:rerun-if-env-changed=JILO_REQUIRE_WINRES");
    println!("cargo:rerun-if-env-changed=WINDRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let icon = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brand/jilo/jilo-studio.ico");
    let result = if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") { windres(&icon) } else { windows::embed_resources(&icon) };
    if let Err(e) = result {
        if std::env::var_os("JILO_REQUIRE_WINRES").is_some() {
            panic!("embedding Windows resources failed: {e}");
        }
        println!("cargo:warning=jilo-studio.exe built without icon/version resources: {e}");
    }
}

/// `a.b.c` of the package version as the four numbers of a Windows file version.
fn file_version() -> String {
    let n = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u16>().ok()).unwrap_or(0);
    format!("{}, {}, {}, 0", n("CARGO_PKG_VERSION_MAJOR"), n("CARGO_PKG_VERSION_MINOR"), n("CARGO_PKG_VERSION_PATCH"))
}

/// The resource script: the icon and the version block Explorer shows in Properties ▸ Details.
fn resource_script(icon: &Path) -> String {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let icon = icon.to_string_lossy().replace('\\', "/");
    let numbers = file_version();
    format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numbers}
PRODUCTVERSION {numbers}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "{COMPANY}"
      VALUE "FileDescription", "{PRODUCT}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "jilo-studio"
      VALUE "LegalCopyright", "{COPYRIGHT}"
      VALUE "OriginalFilename", "jilo-studio.exe"
      VALUE "ProductName", "{PRODUCT}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    )
}

/// MinGW: `windres` compiles the script into an object linked into the app's binary.
fn windres(icon: &Path) -> Result<(), String> {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("no OUT_DIR")?);
    let rc = out.join("jilo-studio.rc");
    let obj = out.join("jilo-studio-res.o");
    std::fs::write(&rc, resource_script(icon)).map_err(|e| format!("{}: {e}", rc.display()))?;
    let tool = std::env::var_os("WINDRES").unwrap_or_else(|| "windres".into());
    let status = std::process::Command::new(&tool)
        .arg(&rc)
        .args(["--output-format=coff", "-o"])
        .arg(&obj)
        .status()
        .map_err(|e| format!("{}: {e} (MinGW-w64's windres must be on PATH)", tool.to_string_lossy()))?;
    if !status.success() {
        return Err(format!("windres exited with {status}"));
    }
    println!("cargo:rustc-link-arg-bin=jilo-studio={}", obj.display());
    Ok(())
}

#[cfg(windows)]
mod windows {
    pub fn embed_resources(icon: &std::path::Path) -> Result<(), String> {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(&icon.to_string_lossy())
            .set("ProductName", super::PRODUCT)
            .set("FileDescription", super::PRODUCT)
            .set("CompanyName", super::COMPANY)
            .set("LegalCopyright", super::COPYRIGHT)
            .set("OriginalFilename", "jilo-studio.exe")
            .set("InternalName", "jilo-studio");
        res.compile().map_err(|e| e.to_string())
    }
}

/// Cross-compiling for Windows (MSVC) from macOS or Linux: `winresource` is not available on this host.
#[cfg(not(windows))]
mod windows {
    pub fn embed_resources(_: &std::path::Path) -> Result<(), String> {
        Err("cross-compiled from a non-Windows host".into())
    }
}
