pub mod cache;
pub mod debugger;
pub mod detect;
mod script;
pub mod system_libs;
pub mod template;
pub mod tool;

pub use cache::{GlobalConfig, ToolchainCache, freight_home};
pub use debugger::{
    DebuggerTemplate, DetectedDebugger,
    detect_debuggers, load_debugger_templates,
};
pub use detect::{
    DetectedCompiler, DetectedToolchain, ToolchainGroups,
    detect_all, detect_all_cached, group_into_toolchains,
    load_templates, load_all_templates, templates_dir, user_templates_dir, toolchain_add, toolchain_use,
    check_manifest_version_bounds,
};
pub use template::{BuildSettings, CompilerTemplate, ModuleStyle};
pub use system_libs::{SystemLibStub, load_system_lib_stubs, find_stub};
pub use tool::{
    DetectedTool, ToolTemplate,
    collect_sources, detect_tools,
    load_formatter_templates, load_linter_templates,
    select_formatter, select_linter,
};
