kind          = "formatter";
name          = "uncrustify";
family        = "uncrustify";
binary        = "uncrustify";
version_arg   = "--version";
version_regex = "Uncrustify-(\\d+\\.\\d+\\.\\d+)";
extensions    = [".cpp", ".cc", ".cxx", ".c++", ".c", ".h", ".hpp", ".hxx", ".cs", ".java"];

// fix:   reformat files in-place (language guessed from extension)
// check: exit non-zero if any file would be changed
run["fix"]   = "--no-backup --replace";
run["check"] = "--check";

// Named settings resolved from [formatter] in freight.toml.
settings["config"] = "-c {value}"; // path to uncrustify.cfg

fn check() {
    find_tool("uncrustify") != ()
}
