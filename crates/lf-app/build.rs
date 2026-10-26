//! Windows: embeds the installer icon (`packaging/icons/app.ico`) and the version info
//! (product name, version from Cargo.toml, copyright) into the .exe, so Explorer, the taskbar
//! and the file's Properties show them. Other targets: nothing to do.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resources();
    }
}

#[cfg(windows)]
fn windows_resources() {
    const ICON: &str = "../../packaging/icons/app.ico";
    println!("cargo:rerun-if-changed={ICON}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ICON)
        .set("ProductName", "lensflow-ImageEditor")
        .set("FileDescription", "lensflow-ImageEditor")
        .set("CompanyName", "LensFlow")
        .set(
            "LegalCopyright",
            "Copyright 2026 LensFlow. Apache License 2.0.",
        )
        .set("OriginalFilename", "lensflow-image-editor.exe")
        .set("InternalName", "lensflow-image-editor");
    res.compile()
        .expect("embed the Windows icon and version info");
}
