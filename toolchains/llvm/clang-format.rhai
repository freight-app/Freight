kind          = "formatter";
name          = "clang-format";
family        = "llvm";
binary        = "clang-format";
version_arg   = "--version";
version_regex = "clang-format version (\\d+\\.\\d+\\.\\d+)";
extensions    = [".cpp", ".cc", ".cxx", ".c++", ".cppm", ".c", ".h", ".hpp", ".hxx", ".cu", ".hip"];

// fix:   format files in place
// check: exit non-zero if any file would be changed (CI use)
run["fix"]   = "-i";
run["check"] = "--dry-run --Werror";

// Named settings resolved from [formatter] in freight.toml.
settings["style"]  = "--style={value}";        // Google | LLVM | Mozilla | WebKit | file
settings["config"] = "--style=file:{value}";   // path to .clang-format config file

// Valid values for each setting (used by LSP completions and help output).
values["style"] = ["Google", "LLVM", "Mozilla", "WebKit", "Chromium", "Microsoft", "GNU", "file"];

fn check() {
    find_tool("clang-format") != ()
}
