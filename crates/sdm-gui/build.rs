//! Windows: embed the app icon and version info in the executable, so Explorer,
//! shortcuts and Apps & features show SDM's mark. Compiles a generated .rc with
//! the Windows SDK's rc.exe (no extra crates); without it the build still
//! succeeds, just without the resource.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !windows_msvc {
        return;
    }
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest_dir.join("..").join("..").join("sdm.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts = version.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    let mut next = || parts.next().unwrap_or(0);
    let (major, minor, patch) = (next(), next(), next());
    let author = std::env::var("CARGO_PKG_AUTHORS").unwrap_or_default();
    // Product name and version match the installer's: release signing checks
    // both against the release (SignPath artifact configurations).
    let script = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3F
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "CompanyName", "{author}"
            VALUE "FileDescription", "SDM - Segmented Download Manager"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "SDM"
            VALUE "LegalCopyright", "Copyright (c) 2026 {author}. MIT License."
            VALUE "OriginalFilename", "SDM.exe"
            VALUE "ProductName", "SDM"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#,
        icon = icon.display().to_string().replace('\\', "\\\\"),
    );

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let (rc_file, res_file) = (out.join("sdm.rc"), out.join("sdm.res"));
    std::fs::write(&rc_file, script).expect("write sdm.rc");
    // Fall back to rc.exe on PATH (a Visual Studio developer prompt).
    let rc = find_rc().unwrap_or_else(|| PathBuf::from("rc.exe"));
    let compiled = Command::new(rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_file)
        .arg(&rc_file)
        .status();
    match compiled {
        Ok(s) if s.success() => println!("cargo:rustc-link-arg-bins={}", res_file.display()),
        _ => println!("cargo:warning=rc.exe unavailable; building without the icon/version resource"),
    }
}

/// The newest Windows SDK's rc.exe.
fn find_rc() -> Option<PathBuf> {
    let root = std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
    let mut versions: Vec<PathBuf> = std::fs::read_dir(root.join(r"Windows Kits\10\bin"))
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    versions.sort();
    versions
        .iter()
        .rev()
        .flat_map(|v| ["x64", "x86"].map(|arch| v.join(arch).join("rc.exe")))
        .find(|p| p.is_file())
}
