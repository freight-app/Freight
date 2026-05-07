// YASM — NASM-compatible x86/x86_64 assembler.
// Drop-in replacement for NASM; accepts the same -f output format flags.

include "_asm-base";

name          = "yasm";
homepage      = "https://yasm.tortall.net/";
binary        = "yasm";
version_regex = "yasm (\\d+\\.\\d+\\.\\d+)";

toolset["as"] = "yasm";

dbg["true"] = "-g dwarf2";

// ── language_option handlers ──────────────────────────────────────────────────
// Declare options users can set under [language.asm] in freight.toml.

// [language.asm] arch = "x86_64"  →  error if effective target arch differs
language_option("arch", |ctx| {
    if ctx.arch != ctx.value {
        return "assembler requires arch '" + ctx.value +
               "' but the effective target is '" + ctx.arch + "'";
    }
    ""
});

fn check() {
    let h = arch;
    if h != "x86_64" && h != "x86" { return false; }
    find_tool("yasm") != ()
}
