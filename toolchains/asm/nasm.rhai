// NASM assembler — x86/x86_64 assembly.

include "_asm-base";

name          = "nasm";
homepage      = "https://www.nasm.us/";
binary        = "nasm";
version_regex = "NASM version (\\d+\\.\\d+(?:\\.\\d+)?)";

toolset["as"] = "nasm";

dbg["true"] = "-g -F dwarf";

warnings["error"] = "-w+all -w+error";   // NASM: -w+error, not -Werror

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
    find_tool("nasm") != ()
}
