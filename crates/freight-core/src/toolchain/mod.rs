pub mod cache;
pub mod debugger;
pub mod detect;
mod script;
pub mod template;

pub use cache::{GlobalConfig, ToolchainCache, freight_home};
pub use debugger::{
    DebuggerTemplate, DetectedDebugger,
    debuggers_dir, detect_debuggers, load_debugger_templates,
};
pub use detect::{
    DetectedCompiler, DetectedToolchain, ToolchainGroups,
    detect_all, detect_all_cached, group_into_toolchains,
    load_templates, load_all_templates, templates_dir, user_templates_dir, toolchain_add, toolchain_use,
};
pub use template::{BuildSettings, CompilerTemplate, ModuleStyle};
