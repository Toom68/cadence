fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "Cadence")
            .set("FileDescription", "Cadence music sequencer")
            .set("CompanyName", "Cadence")
            .set("LegalCopyright", "Copyright (c) Cadence contributors");
        if let Err(err) = res.compile() {
            println!("cargo:warning=could not embed Windows icon resource: {err}");
        }
    }
}
