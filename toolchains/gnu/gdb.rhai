kind          = "debugger";
name          = "gdb";
binary        = "gdb";
version_arg   = "--version";
version_regex = "GNU gdb[^\\d]+(\\d+\\.\\d+)";

// gdb --args <binary> [args]
launch["separator"] = "--args";

// GDB gained native DAP support in version 14 (--interpreter=dap).
// Probed as a fallback; empty = not available as a standalone adapter binary.
dap["binaries"]    = [];
dap["vscode_type"] = "cppdbg";
dap["mi_mode"]     = "gdb";

// Named settings the user can enable via [debugger.settings] in freight.toml.
settings["tui"]    = "--tui";     // full-screen TUI mode
settings["quiet"]  = "-q";       // suppress banner
settings["batch"]  = "--batch";  // non-interactive batch mode
