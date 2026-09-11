// Generate protobuf messages that steam-vent-proto does not ship, with pure-Rust
// rust-protobuf codegen — no protoc required. Each proto lands in its own
// `$OUT_DIR/<name>/` and is `include!`d from the matching src/steam_client module.
fn main() {
    for (proto, out) in [
        ("proto/service_cloudconfigstore.proto", "cloudconfig"),
        ("proto/service_storequery.proto", "storequery"),
        ("proto/service_wishlist.proto", "wishlist"),
    ] {
        println!("cargo:rerun-if-changed={proto}");
        protobuf_codegen::Codegen::new()
            .pure()
            .include("proto")
            .input(proto)
            .cargo_out_dir(out)
            .run_from_script();
    }
}
