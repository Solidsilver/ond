//! On Windows: embeds the icon and version info in `Ond.exe`. Explorer, the
//! taskbar and the media controls take the app's name and icon from these.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../packaging/windows/ond.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../packaging/windows/ond.ico")
            .set("ProductName", "Ond")
            .set("FileDescription", "Ond")
            .set("LegalCopyright", "Ond contributors. MIT OR Apache-2.0");
        if let Err(e) = res.compile() {
            println!("cargo:warning=no Windows resources embedded: {e}");
        }
    }
}
