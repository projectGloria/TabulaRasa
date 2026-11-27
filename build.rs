fn main() {
    println!("cargo:rerun-if-changed=Tabula Rasa.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("Tabula Rasa.ico");
        res.set("ProductName", "TabulaRasa");
        res.set("FileDescription", "TabulaRasa");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed icon: {e}");
        }
    }
}
