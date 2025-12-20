fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    println!("cargo:rerun-if-changed=assets/lol_plugin.rc");
    println!("cargo:rerun-if-changed=assets/lol_plugin.ico");
    println!("cargo:rerun-if-changed=assets/lol_plugin.manifest");

    embed_resource::compile("assets/lol_plugin.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
