//! Embed the existing transparent product icon into the Windows executable.
//! Uses the Windows SDK resource compiler already supplied by the build kit.
use std::{env, fs, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/theme/aetherdesk.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    assert_eq!(
        env::var("CARGO_CFG_TARGET_ENV").as_deref(),
        Ok("msvc"),
        "Windows packaging requires the MSVC toolchain"
    );
    let icon = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("assets/theme/aetherdesk.ico");
    let icon = icon
        .to_str()
        .expect("Icon path must be Unicode")
        .replace('\\', "/");
    assert!(
        !icon.contains(['\"', '\r', '\n']),
        "Invalid Windows resource path"
    );
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let rc = out.join("aetherdesk.rc");
    let res = out.join("aetherdesk.res");
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numbers: Vec<u16> = version
        .split('.')
        .take(3)
        .map(|v| v.parse().expect("Numeric release version"))
        .collect();
    assert_eq!(numbers.len(), 3);
    fs::write(
        &rc,
        format!(
            r#"LANGUAGE 9, 1
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x00040004
FILETYPE 1
BEGIN
 BLOCK "StringFileInfo"
 BEGIN
  BLOCK "040904B0"
  BEGIN
   VALUE "FileDescription", "AetherDesk\0"
   VALUE "ProductName", "AetherDesk\0"
   VALUE "OriginalFilename", "AetherDesk.exe\0"
   VALUE "FileVersion", "{version}\0"
   VALUE "ProductVersion", "{version}\0"
  END
 END
 BLOCK "VarFileInfo"
 BEGIN
  VALUE "Translation", 0x0409, 1200
 END
END
"#,
            major = numbers[0],
            minor = numbers[1],
            patch = numbers[2]
        ),
    )
    .expect("Write Windows resource source");
    let status = Command::new("rc.exe")
        .arg("/nologo")
        .arg("/c65001")
        .arg(format!("/fo{}", res.display()))
        .arg(&rc)
        .status()
        .expect(
            "Windows SDK rc.exe must be available; run through the build kit's MSVC environment",
        );
    assert!(
        status.success() && res.is_file(),
        "Windows icon/resource compilation failed"
    );
    println!("cargo:rustc-link-arg-bin=aetherdesk={}", res.display());
}
