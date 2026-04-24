pub mod cache;
pub mod detect;
pub mod template;

pub use cache::{ToolchainCache, crane_home};
pub use detect::{
    DetectedCompiler, detect_all, detect_all_cached,
    load_templates, load_all_templates, templates_dir, user_templates_dir, toolchain_add,
};
pub use template::{BuildSettings, CompilerTemplate, ModuleStyle};
