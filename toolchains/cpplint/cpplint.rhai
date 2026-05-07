kind          = "linter";
name          = "cpplint";
family        = "cpplint";
binary        = "cpplint";
version_arg   = "--version";
version_regex = "cpplint (\\d+\\.\\d+\\.\\d+)";
extensions    = [".cpp", ".cc", ".cxx", ".c++", ".c", ".h", ".hpp", ".hxx"];

// cpplint is analysis only — no auto-fix; both modes run the same checks
run["check"] = "";
run["fix"]   = "";

// Named settings resolved from [linter] in freight.toml.
settings["filter"]     = "--filter={value}";     // e.g. "-whitespace,-legal/copyright"
settings["linelength"] = "--linelength={value}"; // max line length (default: 80)
settings["root"]       = "--root={value}";       // root dir for include guard detection

fn check() {
    find_tool("cpplint") != ()
}
