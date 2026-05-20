//! Build script for mai-api: compiles Proto3 definitions via tonic-build.
//!
//! Generates Rust code from proto/mai.proto into OUT_DIR.
//! The generated code is included in src/grpc/mod.rs via tonic::include_proto!.
//! Also generates a file descriptor set for tonic-reflection.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Tell cargo to re-run if the proto file changes
    println!("cargo:rerun-if-changed=proto/mai.proto");

    // Determine file descriptor set output path for reflection
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    let descriptor_path = out_dir.join("mai_descriptor.bin");

    // Compile proto with tonic-build, including file descriptor set
    tonic_build::configure()
        .build_server(true)
        .build_client(true) // Client stubs needed for integration tests
        .file_descriptor_set_path(&descriptor_path)
        .compile_protos(&["proto/mai.proto"], &["proto/"])?;

    Ok(())
}
