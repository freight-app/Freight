kind          = "linter";
name          = "cppcheck";
family        = "cppcheck";
binary        = "cppcheck";
version_arg   = "--version";
version_regex = "Cppcheck (\\d+\\.\\d+)";
extensions    = [".cpp", ".cc", ".cxx", ".c++", ".cppm", ".c"];

// cppcheck is static analysis only — no auto-fix; both modes run the same analysis
run["check"] = "--error-exitcode=1";
run["fix"]   = "--error-exitcode=1";

// Named settings resolved from [linter] in freight.toml.
settings["enable"]   = "--enable={value}";   // all | warning | style | performance | portability | information
settings["std"]      = "--std={value}";      // c89 | c99 | c11 | c++11 | c++14 | c++17 | c++20
settings["suppress"] = "--suppress={value}"; // e.g. "missingInclude" or "id:file.cpp"
settings["jobs"]     = "-j {value}";         // parallel analysis jobs

// Valid values for each setting (used by LSP completions and help output).
values["enable"] = ["all", "warning", "style", "performance", "portability", "information"];
values["std"]    = ["c89", "c99", "c11", "c17", "c++03", "c++11", "c++14", "c++17", "c++20"];

fn check() {
    find_tool("cppcheck") != ()
}
