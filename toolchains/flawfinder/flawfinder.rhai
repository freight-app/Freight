kind          = "linter";
name          = "flawfinder";
family        = "flawfinder";
binary        = "flawfinder";
version_arg   = "--version";
version_regex = "Flawfinder version (\\d+\\.\\d+)";
extensions    = [".cpp", ".cc", ".cxx", ".c++", ".c", ".h", ".hpp", ".hxx"];

// flawfinder is security analysis only — no auto-fix
// exits non-zero when any hit at or above minlevel is found
run["check"] = "--error-level=1";
run["fix"]   = "--error-level=1";

// Named settings resolved from [linter] in freight.toml.
settings["minlevel"] = "--minlevel={value}"; // minimum risk level to report: 0 (all) – 5 (critical only), default 1

// Valid values for each setting (used by LSP completions and help output).
values["minlevel"] = ["0", "1", "2", "3", "4", "5"];

fn check() {
    find_tool("flawfinder") != ()
}
