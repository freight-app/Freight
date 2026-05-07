// YASM — NASM-compatible x86/x86_64 assembler.
// Drop-in replacement for NASM; accepts the same -f output format flags.

include "_asm-base";

name          = "yasm";
homepage      = "https://yasm.tortall.net/";
binary        = "yasm";
version_regex = "yasm (\\d+\\.\\d+\\.\\d+)";

toolset["as"] = "yasm";

dbg["true"] = "-g dwarf2";

fn check() {
    let h = arch;
    if h != "x86_64" && h != "x86" { return false; }
    find_tool("yasm") != ()
}
