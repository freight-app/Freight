//! Coloured CLI output helpers — used by every `cmd_*` shell so they speak in
//! the same voice. Lives in the binary because the library has no business
//! formatting for a terminal.

use owo_colors::OwoColorize;

pub fn print_success(msg: &str) {
    println!("{} {}", "✓".green().bold(), msg);
}

pub fn print_warning(msg: &str) {
    eprintln!("{} {}", "⚠".yellow().bold(), msg);
}

pub fn print_error(msg: &str) {
    eprintln!("{} {}", "✗".red().bold(), msg);
}

pub fn print_status(verb: &str, detail: &str) {
    println!("{:>12} {}", verb.cyan().bold(), detail);
}
