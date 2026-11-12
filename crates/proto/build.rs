//! Generates the Rust code for `proto/link.proto`. protox compiles the
//! file in Rust, so building needs no `protoc` (CI runners and the cross
//! images don't have it).

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = protox::compile(["proto/link.proto"], ["proto"])?;
    tonic_prost_build::configure().compile_fds(files)?;
    println!("cargo:rerun-if-changed=proto");
    Ok(())
}
