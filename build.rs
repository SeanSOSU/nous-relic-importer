// The app's own icon and version details.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "StarRailSim Sync")
            .set("FileDescription", "StarRailSim Sync")
            .set("InternalName", "starrailsim-sync")
            .set("OriginalFilename", "starrailsim-sync.exe");
        res.compile().expect("compiling the Windows resources");
    }
}
