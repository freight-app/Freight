kind          = "formatter";
name          = "astyle";
family        = "astyle";
binary        = "astyle";
version_arg   = "--version";
version_regex = "Artistic Style Version (\\d+\\.\\d+)";
extensions    = [".cpp", ".cc", ".cxx", ".c++", ".c", ".h", ".hpp", ".hxx", ".cs", ".java"];

// fix:   format files in-place, no backup
// check: show what would change without writing (note: exits 0 regardless — use clang-format for CI checks)
run["fix"]   = "--suffix=none";
run["check"] = "--dry-run --suffix=none";

// Named settings resolved from [formatter] in freight.toml.
settings["style"]  = "--style={value}";         // allman | java | kr | stroustrup | google | mozilla | otbs | vtk | ratliff | lisp
settings["indent"] = "--indent=spaces={value}"; // indentation width in spaces

// Valid values for each setting (used by LSP completions and help output).
values["style"] = ["allman", "java", "kr", "stroustrup", "google", "mozilla", "otbs", "vtk", "ratliff", "lisp"];

fn check() {
    find_tool("astyle") != ()
}
