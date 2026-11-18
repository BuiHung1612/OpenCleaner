// Windows: embed the app icon and the manifest (per-monitor DPI, common controls v6).
fn main() {
    println!("cargo:rerun-if-changed=app.ico");
    println!("cargo:rerun-if-changed=OpenCleaner.manifest");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("app.ico").set_manifest_file("OpenCleaner.manifest");
        res.set("ProductName", "OpenCleaner").set("FileDescription", "OpenCleaner");
        res.compile().expect("windows resources");
    }
}
