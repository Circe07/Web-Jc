fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=ORQ_FIXTURE_BUILT=yes");
}
