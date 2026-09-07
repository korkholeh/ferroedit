//! Bakes the syntax set into the binary at compile time.
//!
//! syntect can build a `SyntaxSet` at startup, but linking the contexts of ~80
//! grammars costs ~105 ms — a visible pause before the first frame. Dumping the
//! linked set here and loading it back with `from_binary` costs ~1.5 ms
//! instead, and it is the only way to add a grammar of our own without paying
//! that link cost on every run (ADR-022).

use std::path::PathBuf;

use syntect::dumps;
use syntect::parsing::SyntaxSet;

fn main() {
    println!("cargo:rerun-if-changed=assets/syntaxes");
    println!("cargo:rerun-if-changed=build.rs");

    // Anchored to the manifest rather than to the working directory: cargo runs
    // a build script from the package root today, and that is a convention
    // rather than a guarantee.
    let syntaxes = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"))
        .join("assets/syntaxes");

    // `load_defaults_newlines`, not `nonewlines`: the parser is fed lines with
    // their terminator, which is what lets `$`-anchored rules in a grammar
    // match at all.
    let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
    builder
        .add_from_folder(&syntaxes, true)
        .expect("assets/syntaxes contains loadable .sublime-syntax files");
    let syntax_set = builder.build();

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"))
        .join("syntaxes.packdump");
    dumps::dump_to_file(&syntax_set, &out).expect("the syntax dump is writable");
}
