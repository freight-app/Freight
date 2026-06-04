use clap::{Command, ValueEnum};
use freight::toolchain::{detect_all_cached, group_into_toolchains, load_all_templates};
use std::io::{self, Write};

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum CompletionShell {
    Bash,
    Elvish,
    Fish,
    #[value(name = "powershell", alias = "power-shell")]
    PowerShell,
    Zsh,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum CompletionContext {
    /// Dependency keys accepted in inline dependency tables and by `freight add` flags.
    #[value(name = "add-dep-keys")]
    AddDepKeys,
    /// Detected primary toolchain names accepted by `freight toolchain use`.
    #[value(name = "toolchain-use")]
    ToolchainUse,
}

const DEP_KEYS: &[&str] = &[
    "version", "path", "git", "branch", "tag", "rev", "features", "type", "include",
];

pub(crate) fn print_completion_candidates(context: CompletionContext) {
    let candidates = completion_candidates(context);
    if !candidates.is_empty() {
        let mut stdout = io::stdout();
        for candidate in candidates {
            let _ = writeln!(stdout, "{candidate}");
        }
    }
}

fn completion_candidates(context: CompletionContext) -> Vec<String> {
    match context {
        CompletionContext::AddDepKeys => DEP_KEYS.iter().map(|key| (*key).to_string()).collect(),
        CompletionContext::ToolchainUse => detected_toolchain_names(),
    }
}

fn detected_toolchain_names() -> Vec<String> {
    let templates = load_all_templates();
    if templates.is_empty() {
        return Vec::new();
    }
    let detected = detect_all_cached(&templates);
    let groups = group_into_toolchains(detected);
    groups.toolchains.into_iter().map(|tc| tc.name).collect()
}

pub(crate) fn print_completion(shell: CompletionShell, cmd: &Command) {
    let script = match shell {
        CompletionShell::Bash => bash_completion(cmd),
        CompletionShell::Elvish => elvish_completion(cmd),
        CompletionShell::Fish => fish_completion(cmd),
        CompletionShell::PowerShell => powershell_completion(cmd),
        CompletionShell::Zsh => zsh_completion(cmd),
    };
    let _ = io::stdout().write_all(script.as_bytes());
}

fn bash_completion(cmd: &Command) -> String {
    let top = command_names(cmd).join(" ");
    let top_opts = option_names(cmd).join(" ");
    let mut cases = String::new();
    for sub in cmd.get_subcommands() {
        let name = sub.get_name();
        let opts = merged_options(cmd, sub).join(" ");
        let nested = command_names(sub).join(" ");
        cases.push_str(&format!(
            "        {name}) opts='{opts}' ; subcmds='{nested}' ;;\n",
            name = shell_quote(name),
            opts = shell_quote(&opts),
            nested = shell_quote(&nested)
        ));
    }

    format!(
        r#"# bash completion for freight
_freight()
{{
    local cur cmd opts subcmds
    COMPREPLY=()
    cur="${{COMP_WORDS[COMP_CWORD]}}"

    if [[ $COMP_CWORD -le 1 ]]; then
        if [[ $cur == -* ]]; then
            COMPREPLY=( $(compgen -W '{top_opts}' -- "$cur") )
        else
            COMPREPLY=( $(compgen -W '{top} {top_opts}' -- "$cur") )
        fi
        return 0
    fi

    cmd="${{COMP_WORDS[1]}}"
    opts='{top_opts}'
    subcmds=''
    case "$cmd" in
{cases}        *) ;;
    esac

    if [[ $cmd == add && $cur != -* ]]; then
        COMPREPLY=( $(compgen -W "$(freight __complete add-dep-keys 2>/dev/null) $opts" -- "$cur") )
        return 0
    fi

    if [[ $cmd == toolchain && ${{COMP_CWORD}} -ge 3 && ${{COMP_WORDS[2]}} == use && $cur != -* ]]; then
        COMPREPLY=( $(compgen -W "$(freight __complete toolchain-use 2>/dev/null)" -- "$cur") )
        return 0
    fi

    if [[ $cur == -* ]]; then
        COMPREPLY=( $(compgen -W "$opts" -- "$cur") )
    else
        COMPREPLY=( $(compgen -W "$subcmds $opts" -- "$cur") )
    fi
}}
complete -F _freight freight
"#,
        top = top,
        top_opts = top_opts,
        cases = cases
    )
}

fn zsh_completion(cmd: &Command) -> String {
    let top = command_names(cmd).join(" ");
    let top_opts = option_names(cmd).join(" ");
    let mut cases = String::new();
    for sub in cmd.get_subcommands() {
        let name = sub.get_name();
        let opts = merged_options(cmd, sub).join(" ");
        let nested = command_names(sub).join(" ");
        cases.push_str(&format!(
            "      {name}) opts=({opts}); subcmds=({nested}) ;;\n",
            name = shell_quote(name),
            opts = words_for_zsh(&opts),
            nested = words_for_zsh(&nested)
        ));
    }
    format!(
        r#"#compdef freight
# zsh completion for freight
_freight() {{
  local -a opts subcmds
  opts=({top_opts})
  subcmds=({top})

  if (( CURRENT > 2 )); then
    case "${{words[2]}}" in
{cases}      *) ;;
    esac
  fi

  if [[ ${{words[2]}} == add && ${{words[CURRENT]}} != -* ]]; then
    compadd -- ${{(f)"$(freight __complete add-dep-keys 2>/dev/null)"}} $opts
    return
  fi

  if [[ ${{words[2]}} == toolchain && ${{words[3]}} == use && ${{words[CURRENT]}} != -* ]]; then
    compadd -- ${{(f)"$(freight __complete toolchain-use 2>/dev/null)"}}
    return
  fi

  if [[ ${{words[CURRENT]}} == -* ]]; then
    compadd -- $opts
  else
    compadd -- $subcmds $opts
  fi
}}
_freight "$@"
"#,
        top_opts = words_for_zsh(&top_opts),
        top = words_for_zsh(&top),
        cases = cases
    )
}

fn fish_completion(cmd: &Command) -> String {
    let mut out = String::from("# fish completion for freight\n");
    for arg in cmd.get_arguments() {
        push_fish_option(&mut out, None, arg);
    }
    for sub in cmd.get_subcommands() {
        let name = sub.get_name();
        out.push_str(&format!(
            "complete -c freight -n '__fish_use_subcommand' -a '{}' -d '{}'\n",
            escape_single(name),
            escape_single(&help_text(sub.get_about()))
        ));
        for arg in sub.get_arguments() {
            push_fish_option(&mut out, Some(name), arg);
        }
        for nested in sub.get_subcommands() {
            out.push_str(&format!(
                "complete -c freight -n '__fish_seen_subcommand_from {}' -a '{}' -d '{}'\n",
                escape_single(name),
                escape_single(nested.get_name()),
                escape_single(&help_text(nested.get_about()))
            ));
        }
    }
    out.push_str("complete -c freight -n '__fish_seen_subcommand_from add; and not __fish_seen_argument -s h -l help' -a '(freight __complete add-dep-keys 2>/dev/null)' -d 'dependency key'\n");
    out.push_str("complete -c freight -n '__fish_seen_subcommand_from toolchain; and __fish_seen_subcommand_from use' -a '(freight __complete toolchain-use 2>/dev/null)' -d 'detected toolchain'\n");
    out
}

fn powershell_completion(cmd: &Command) -> String {
    let commands = all_command_names(cmd).join(" ");
    let options = all_options(cmd).join(" ");
    format!(
        r#"# PowerShell completion for freight
Register-ArgumentCompleter -Native -CommandName freight -ScriptBlock {{
    param($wordToComplete, $commandAst, $cursorPosition)
    $commands = '{commands}'.Split(' ')
    $options = '{options}'.Split(' ')
    $candidates = if ($wordToComplete.StartsWith('-')) {{ $options }} else {{ $commands + $options }}
    $candidates | Where-Object {{ $_ -like "$wordToComplete*" }} | ForEach-Object {{
        [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
    }}
}}
"#
    )
}

fn elvish_completion(cmd: &Command) -> String {
    let candidates = all_words(cmd).join(" ");
    format!(
        r#"# elvish completion for freight
set edit:completion:arg-completer[freight] = {{|@words|
    var candidates = [{candidates}]
    put $@candidates
}}
"#
    )
}

fn command_names(cmd: &Command) -> Vec<String> {
    cmd.get_subcommands()
        .filter(|sub| !sub.is_hide_set())
        .map(|sub| sub.get_name().to_string())
        .collect()
}

fn all_command_names(cmd: &Command) -> Vec<String> {
    let mut commands = command_names(cmd);
    for sub in cmd.get_subcommands() {
        commands.extend(all_command_names(sub));
    }
    commands.sort();
    commands.dedup();
    commands
}

fn option_names(cmd: &Command) -> Vec<String> {
    let mut opts = Vec::new();
    for arg in cmd.get_arguments() {
        if arg.is_hide_set() {
            continue;
        }
        if let Some(short) = arg.get_short() {
            opts.push(format!("-{short}"));
        }
        if let Some(long) = arg.get_long() {
            opts.push(format!("--{long}"));
        }
    }
    opts
}

fn merged_options(root: &Command, cmd: &Command) -> Vec<String> {
    let mut opts = option_names(root);
    opts.extend(option_names(cmd));
    opts.sort();
    opts.dedup();
    opts
}

fn all_options(cmd: &Command) -> Vec<String> {
    let mut opts = option_names(cmd);
    for sub in cmd.get_subcommands() {
        opts.extend(all_options(sub));
    }
    opts.sort();
    opts.dedup();
    opts
}

fn all_words(cmd: &Command) -> Vec<String> {
    let mut words = all_command_names(cmd);
    words.extend(all_options(cmd));
    words.sort();
    words.dedup();
    words
}

fn push_fish_option(out: &mut String, command: Option<&str>, arg: &clap::Arg) {
    if arg.is_hide_set() {
        return;
    }
    let mut line = String::from("complete -c freight");
    if let Some(command) = command {
        line.push_str(&format!(
            " -n '__fish_seen_subcommand_from {}'",
            escape_single(command)
        ));
    }
    if let Some(short) = arg.get_short() {
        line.push_str(&format!(" -s {short}"));
    }
    if let Some(long) = arg.get_long() {
        line.push_str(&format!(" -l {}", escape_single(long)));
    }
    let help = help_text(arg.get_help());
    if !help.is_empty() {
        line.push_str(&format!(" -d '{}'", escape_single(&help)));
    }
    out.push_str(&line);
    out.push('\n');
}

fn help_text(text: Option<&clap::builder::StyledStr>) -> String {
    text.map(ToString::to_string).unwrap_or_default()
}

fn words_for_zsh(words: &str) -> String {
    words
        .split_whitespace()
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(word: &str) -> String {
    word.replace('\\', "\\\\").replace('\'', "'\\''")
}

fn escape_single(word: &str) -> String {
    word.replace('\\', "\\\\").replace('\'', "\\'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_dependency_key_candidates_include_supported_dep_fields() {
        let candidates = completion_candidates(CompletionContext::AddDepKeys);
        for expected in [
            "version", "path", "git", "branch", "tag", "rev", "features", "type", "include",
        ] {
            assert!(candidates.iter().any(|candidate| candidate == expected));
        }
    }

    #[test]
    fn bash_completion_wires_dynamic_contexts_without_exposing_helper_command() {
        let script = bash_completion(&crate::cli_command());
        assert!(script.contains("freight __complete add-dep-keys"));
        assert!(script.contains("freight __complete toolchain-use"));
        assert!(script.contains("toolchain"));
        assert!(!command_names(&crate::cli_command()).contains(&"__complete".to_string()));
    }

    #[test]
    fn zsh_and_fish_completion_wire_dynamic_contexts() {
        let cmd = crate::cli_command();
        let zsh = zsh_completion(&cmd);
        let fish = fish_completion(&cmd);
        assert!(zsh.contains("freight __complete add-dep-keys"));
        assert!(zsh.contains("freight __complete toolchain-use"));
        assert!(fish.contains("freight __complete add-dep-keys"));
        assert!(fish.contains("freight __complete toolchain-use"));
    }
}
