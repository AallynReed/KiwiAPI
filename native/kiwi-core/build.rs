fn main() {
    println!("cargo:rerun-if-changed=csrc/clipper");
    cc::Build::new()
        .cpp(true)
        .file("csrc/clipper/clipper.cpp")
        .file("csrc/clipper/offset.cpp")
        .flag_if_supported("/EHsc")
        .warnings(false)
        .compile("kiwi_clipper");
}
