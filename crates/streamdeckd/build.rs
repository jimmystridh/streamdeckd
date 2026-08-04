use std::env;
use std::path::PathBuf;

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let plist = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"))
        .join("Info.plist");
    println!("cargo:rerun-if-changed={}", plist.display());
    println!(
        "cargo:rustc-link-arg-bin=streamdeckd=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist.display()
    );
}
