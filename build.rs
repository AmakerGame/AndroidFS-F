// Embeds the application icon and version info into androidfs_driver.exe.
// Only runs for Windows targets (the Android daemon build is skipped).
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");

    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("assets/icon.ico");
            res.set("ProductName", "AndroidFS-F");
            res.set(
                "FileDescription",
                "Mount an Android device as a Windows drive over ADB",
            );
            res.set("LegalCopyright", "GPL-3.0-only");
            if let Err(err) = res.compile() {
                println!("cargo:warning=could not embed the application icon: {err}");
            }
        }
    }
}
