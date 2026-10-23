// FreeType 2.14.3, the version Pillow 13's wheels bundle, built from the vendored
// sources with the stock options (csrc/ overrides only the module list and the
// compressed-font switches). Pillow's build also links HarfBuzz into the
// auto-hinter, where it classifies glyphs reached only through GSUB; basic
// layout draws cmap glyphs, which hint the same (checked against Pillow on the
// unhinted Noto Thai faces, GSUB/GPOS intact).
fn main() {
    let ft = std::path::Path::new("freetype");
    let mut b = cc::Build::new();
    b.include("csrc").include(ft.join("include")).define("FT2_BUILD_LIBRARY", None).warnings(false);
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        b.define("_CRT_SECURE_NO_WARNINGS", None);
    }
    for f in [
        "src/base/ftsystem.c",
        "src/base/ftinit.c",
        "src/base/ftdebug.c",
        "src/base/ftbase.c",
        "src/base/ftbitmap.c",
        "src/base/ftglyph.c",
        "src/base/ftmm.c",
        "src/sfnt/sfnt.c",
        "src/truetype/truetype.c",
        "src/cff/cff.c",
        "src/psaux/psaux.c",
        "src/pshinter/pshinter.c",
        "src/autofit/autofit.c",
        "src/smooth/smooth.c",
        "src/psnames/psnames.c",
    ] {
        b.file(ft.join(f));
    }
    b.file("csrc/shim.c");
    b.compile("kiwi_freetype");
    println!("cargo:rerun-if-changed=csrc");
    println!("cargo:rerun-if-changed=freetype");
}
