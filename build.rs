// Generate the CloudConfigStore protobuf messages (used by library collections) with
// pure-Rust rust-protobuf codegen — no protoc required. The generated code lands in
// `$OUT_DIR/cloudconfig/` and is `include!`d from src/steam_client/cloudconfig.rs.
fn main() {
    println!("cargo:rerun-if-changed=proto/service_cloudconfigstore.proto");
    protobuf_codegen::Codegen::new()
        .pure()
        .include("proto")
        .input("proto/service_cloudconfigstore.proto")
        .cargo_out_dir("cloudconfig")
        .run_from_script();
}
