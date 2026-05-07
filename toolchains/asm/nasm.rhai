// NASM assembler — x86/x86_64 assembly.

include "_asm-base";

name          = "nasm";
homepage      = "https://www.nasm.us/";
binary        = "nasm";
version_regex = "NASM version (\\d+\\.\\d+(?:\\.\\d+)?)";

toolset["as"] = "nasm";

dbg["true"] = "-g -F dwarf";

fn check() {
    let h = arch;
    if h != "x86_64" && h != "x86" { return false; }
    find_tool("nasm") != ()
}
