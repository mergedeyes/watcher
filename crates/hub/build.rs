fn main() {
    // Rebuild when the web UI was rebuilt, so release binaries embed the
    // current files (cargo scans the whole directory).
    println!("cargo:rerun-if-changed=../../web/build");
}
