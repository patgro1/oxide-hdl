use std::path::PathBuf;

fn main() {
    let dir: PathBuf = ["headers", "tree-sitter-vhdl", "src"].iter().collect();

    cc::Build::new()
        .include(&dir)
        .file(dir.join("parser.c"))
        .compile("tree-sitter-vhdl");
}
