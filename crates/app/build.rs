//! On Windows, embeds the app icon (and version metadata) in the .exe, so
//! Explorer and the taskbar show it. Only when building on Windows: the
//! resource compiler is a Windows tool, and cross-checking from another
//! system (`cargo check --target x86_64-pc-windows-msvc`) must not need it.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/icon.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon/icon.ico");
        res.set("ProductName", "Voxferro");
        res.set("FileDescription", "Voxferro voxel editor");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the icon: {e}");
        }
    }
}
