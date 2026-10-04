fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    kinetic_buildinfo::emit();
}
