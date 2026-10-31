fn main() {
    // .rc 引用的 manifest、图标改了也要重新编译资源
    println!("cargo:rerun-if-changed=assets");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_resource::compile("assets/jie.rc", embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}
