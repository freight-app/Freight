kind          = "formatter";
name          = "fprettify";
family        = "fprettify";
binary        = "fprettify";
version_arg   = "--version";
version_regex = "fprettify (\\d+\\.\\d+\\.\\d+)";
extensions    = [".f90", ".f95", ".f03", ".f08", ".F90", ".F95", ".f"];

// fix:   format files in-place
// check: exit non-zero if any file needs reformatting (requires fprettify >= 0.3.7)
run["fix"]   = "-i";
run["check"] = "--check";

// Named settings resolved from [formatter] in freight.toml.
settings["indent"] = "--indent {value}"; // indentation width in spaces (default: 3)
settings["config"] = "-c {value}";       // path to .fprettify config file

fn check() {
    find_tool("fprettify") != ()
}
