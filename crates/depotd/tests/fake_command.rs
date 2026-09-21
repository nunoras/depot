use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const DIRECTORY: &str = "DEPOT_FAKE_DIR";

fn main() -> ExitCode {
    let Some(root) = env::var_os(DIRECTORY).map(PathBuf::from) else {
        return ExitCode::SUCCESS;
    };
    let arguments: Vec<String> = env::args().skip(1).collect();
    if let Err(error) = record(&root, &arguments) {
        eprintln!("fake command: {error}");
        return ExitCode::FAILURE;
    }
    respond(&root, &arguments)
}

fn record(root: &Path, arguments: &[String]) -> io::Result<()> {
    let mut line = arguments.join("\n");
    line.push('\n');
    line.push('\n');
    let mut calls = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("calls.txt"))?;
    calls.write_all(line.as_bytes())
}

fn respond(root: &Path, arguments: &[String]) -> ExitCode {
    let key = arguments.first().map(String::as_str).unwrap_or("default");
    if let Ok(text) = fs::read_to_string(root.join(format!("{key}.stdout"))) {
        print!("{text}");
    }
    let _ = io::stdout().flush();
    if let Ok(text) = fs::read_to_string(root.join(format!("{key}.stderr"))) {
        eprint!("{text}");
    }
    let _ = io::stderr().flush();
    let code = fs::read_to_string(root.join(format!("{key}.exit")))
        .ok()
        .and_then(|text| text.trim().parse::<u8>().ok())
        .unwrap_or(0);
    ExitCode::from(code)
}
