kind          = "debugger";
name          = "lldb";
binary        = "lldb";
version_arg   = "--version";
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b";

// lldb -- <binary> [args]
launch["separator"] = "--";

// DAP adapter binary names probed in order.
// lldb-dap is the name from LLVM 18+; lldb-vscode on older installations.
dap["binaries"]    = ["lldb-dap", "lldb-vscode"];
dap["vscode_type"] = "lldb";
dap["mi_mode"]     = "lldb";

// Named settings the user can enable via [debugger.settings] in freight.toml.
settings["no_use_colors"] = "--no-use-colors";  // disable ANSI colour output
settings["batch"]         = "--batch";           // non-interactive batch mode
