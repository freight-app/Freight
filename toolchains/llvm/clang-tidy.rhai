kind          = "linter";
name          = "clang-tidy";
family        = "llvm";
binary        = "clang-tidy";
version_arg   = "--version";
version_regex = "LLVM version (\\d+\\.\\d+\\.\\d+)";
extensions    = [".cpp", ".cc", ".cxx", ".c++", ".cppm", ".c"];

// check: report issues (default)
// fix:   auto-apply safe fixes
run["check"] = "";
run["fix"]   = "--fix --fix-errors";

// Named settings resolved from [linter] in freight.toml.
settings["checks"] = "--checks={value}";      // e.g. "-*,modernize-*,bugprone-*"
settings["config"] = "--config-file={value}"; // path to .clang-tidy config file

// checks and config are freeform — no fixed value list.

fn check() {
    find_tool("clang-tidy") != ()
}
