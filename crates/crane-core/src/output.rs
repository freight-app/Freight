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

pub fn print_unimplemented(cmd: &str) {
    println!("{} `crane {}` is not yet implemented", "⚠".yellow().bold(), cmd);
}

pub fn print_status(verb: &str, detail: &str) {
    println!("{:>12} {}", verb.cyan().bold(), detail);
}
