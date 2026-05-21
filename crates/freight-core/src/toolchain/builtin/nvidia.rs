use crate::toolchain::template::{CompilerTemplate, OptionHandler, ToolchainDef, LinkingParams};

fn nvhpc_base(name: &str, binary: &str) -> ToolchainDef {
    let mut d = ToolchainDef {
        name: name.into(),
        binary: binary.into(),
        family: "nvidia".into(),
        version_arg: "--version".into(),
        version_regex: r"(\d+\.\d+)".into(),
        flags_debug: "-g".into(),
        flags_lto: "".into(),
        cpu_ext: "-m{name}".into(),
        supported_archs: vec!["x86_64".into(), "aarch64".into()],
        supported_os: vec!["linux".into()],
        ..Default::default()
    };
    d.flags_opt.insert("0".into(), "-O0".into());
    d.flags_opt.insert("1".into(), "-O1".into());
    d.flags_opt.insert("2".into(), "-O2".into());
    d.flags_opt.insert("3".into(), "-O3".into());
    d.flags_opt.insert("s".into(), "-O2".into());
    d.flags_opt.insert("z".into(), "-O2".into());
    d.flags_warnings.insert("none".into(), "".into());
    d.flags_warnings.insert("default".into(), "".into());
    d.flags_warnings.insert("all".into(), "-Minform=warn".into());
    d.flags_warnings.insert("error".into(), "-Minform=warn -Werror".into());
    d.structure.insert("include_dir".into(), "-I{path}".into());
    d.structure.insert("define".into(), "-D{name}".into());
    d.structure.insert("define_value".into(), "-D{name}={value}".into());
    d.structure.insert("output".into(), "-o {path}".into());
    d.structure.insert("compile_only".into(), "-c".into());
    d.structure.insert("dep_file".into(), "-MMD -MF {path}".into());
    d.toolset.insert("ar".into(), "ar".into());
    d
}

pub fn nvcc() -> CompilerTemplate {
    fn sm_arch_h(v: &str, _: &str, _: &str, _: &str, _: &str) -> Result<Vec<String>, String> {
        if !v.is_empty() { Ok(vec![format!("--gpu-architecture={v}")]) } else { Ok(vec![]) }
    }
    fn lang_arch_h(v: &str, _: &str, arch: &str, _: &str, _: &str) -> Result<Vec<String>, String> {
        if !v.is_empty() && arch != v {
            Err(format!("nvcc requires arch '{v}' but the effective target is '{arch}'"))
        } else {
            Ok(vec![])
        }
    }

    let mut d = ToolchainDef {
        name: "nvcc".into(),
        binary: "nvcc".into(),
        family: "".into(),
        version_arg: "--version".into(),
        version_regex: r"release (\d+\.\d+)".into(),
        extensions: vec![".cu".into(), ".cuh".into()],
        passthrough_enabled: true,
        passthrough_prefix: "-Xcompiler".into(),
        always_flags: vec!["--expt-relaxed-constexpr".into(), "--extended-lambda".into()],
        supported_archs: vec!["x86_64".into(), "aarch64".into()],
        supported_os: vec!["linux".into(), "windows".into()],
        required_tools: vec!["ptxas".into(), "fatbinary".into()],
        requires_toolchain: vec!["cpp".into()],
        flags_debug: "-g -G".into(),
        flags_lto: "".into(),
        ..Default::default()
    };
    d.flags_opt.insert("0".into(), "-O0".into());
    d.flags_opt.insert("1".into(), "-O1".into());
    d.flags_opt.insert("2".into(), "-O2".into());
    d.flags_opt.insert("3".into(), "-O3".into());
    d.flags_opt.insert("s".into(), "-O2".into());
    d.flags_opt.insert("z".into(), "-O2".into());
    // Host-code warnings go through -Xcompiler; device-code warnings use --Werror.
    d.flags_warnings.insert("none".into(), "--diag-suppress all".into());
    d.flags_warnings.insert("default".into(), "-Xcompiler -Wall".into());
    d.flags_warnings.insert("all".into(), "-Xcompiler -Wall,-Wextra --Werror cross-execution-space-call,reorder".into());
    d.flags_warnings.insert("error".into(), "-Xcompiler -Wall,-Wextra,-Werror --Werror all-warnings".into());
    d.standards.insert("c++17".into(), "-std=c++17".into());
    d.standards.insert("c++20".into(), "-std=c++20".into());
    d.structure.insert("include_dir".into(), "-I{path}".into());
    d.structure.insert("define".into(), "-D{name}".into());
    d.structure.insert("define_value".into(), "-D{name}={value}".into());
    d.structure.insert("output".into(), "-o {path}".into());
    d.structure.insert("compile_only".into(), "-c".into());
    d.structure.insert("dep_file".into(), "-MD -MF {path}".into());
    d.toolset.insert("ld".into(), "nvcc".into());
    d.linking.push(("cuda".into(), LinkingParams {
        abi: "cuda".into(),
        compatible: vec!["c++".into(), "c".into(), "fortran".into()],
        linker: "cuda".into(),
        extensions: vec![".cu".into(), ".cuh".into()],
        compile_binary: None,
    }));
    d.compiler_option_handlers.insert("sm_arch".into(), OptionHandler {
        default_value: None,
        callback: sm_arch_h,
    });
    d.language_option_handlers.insert("arch".into(), OptionHandler {
        default_value: None,
        callback: lang_arch_h,
    });
    CompilerTemplate::from_def(d).unwrap()
}

pub fn nvcpp() -> CompilerTemplate {
    let mut d = nvhpc_base("nvc++", "nvc++");
    d.extensions = vec![".cpp".into(), ".cc".into(), ".cxx".into(), ".c++".into()];
    d.sanitizer_options = vec!["address".into()];
    d.standards.insert("c++17".into(), "-std=c++17".into());
    d.standards.insert("c++20".into(), "-std=c++20".into());
    d.toolset.insert("cc".into(), "nvc".into());
    d.toolset.insert("cxx".into(), "nvc++".into());
    d.toolset.insert("ld".into(), "nvc++".into());
    d.linking.push(("cpp".into(), LinkingParams {
        abi: "c++".into(),
        compatible: vec!["c".into(), "fortran".into()],
        linker: "".into(),
        extensions: vec![".cpp".into(), ".cc".into(), ".cxx".into(), ".c++".into()],
        compile_binary: None,
    }));
    CompilerTemplate::from_def(d).unwrap()
}

pub fn nvc() -> CompilerTemplate {
    let mut d = nvhpc_base("nvc", "nvc");
    d.extensions = vec![".c".into()];
    d.sanitizer_options = vec!["address".into()];
    d.standards.insert("c11".into(), "-std=c11".into());
    d.standards.insert("c17".into(), "-std=c17".into());
    d.toolset.insert("cc".into(), "nvc".into());
    d.toolset.insert("ld".into(), "nvc".into());
    d.linking.push(("c".into(), LinkingParams {
        abi: "c".into(),
        compatible: vec!["fortran".into()],
        compile_binary: Some("nvc".into()),
        linker: "".into(),
        extensions: vec![".c".into()],
    }));
    CompilerTemplate::from_def(d).unwrap()
}

pub fn nvfortran() -> CompilerTemplate {
    let mut d = nvhpc_base("nvfortran", "nvfortran");
    d.extensions = vec![".f90".into(), ".f95".into(), ".f".into(), ".F90".into()];
    d.sanitizer_options = vec!["address".into()];
    d.flags_stdlib.clear();
    d.standards.insert("f2003".into(), "-Mstandard".into());
    d.standards.insert("f2008".into(), "-Mstandard".into());
    d.standards.insert("f2018".into(), "-Mstandard".into());
    d.toolset.insert("ld".into(), "nvfortran".into());
    d.linking.push(("fortran".into(), LinkingParams {
        abi: "fortran".into(),
        compatible: vec!["c".into()],
        compile_binary: Some("nvfortran".into()),
        linker: "".into(),
        extensions: vec![".f90".into(), ".f95".into(), ".f".into(), ".F90".into()],
    }));
    CompilerTemplate::from_def(d).unwrap()
}

pub fn templates() -> Vec<CompilerTemplate> {
    vec![nvcc(), nvcpp(), nvc(), nvfortran()]
}
