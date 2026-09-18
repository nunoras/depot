use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const DIRECTORY: &str = "DEPOT_FAKE_BOXR_DIR";
const SEPARATOR: char = '\u{1f}';
const RECORD: char = '\u{1e}';

fn main() -> ExitCode {
    let Some(root) = env::var_os(DIRECTORY).map(PathBuf::from) else {
        return ExitCode::SUCCESS;
    };
    let arguments: Vec<String> = env::args().skip(1).collect();
    if let Err(error) = record(&root, &arguments) {
        eprintln!("fake boxr: {error}");
        return ExitCode::FAILURE;
    }
    respond(&root, &arguments)
}

fn record(root: &Path, arguments: &[String]) -> io::Result<()> {
    let line = arguments
        .iter()
        .map(|argument| format!("{argument}{SEPARATOR}"))
        .collect::<String>();
    let mut calls = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("calls.txt"))?;
    write!(calls, "{line}{RECORD}")
}

fn respond(root: &Path, arguments: &[String]) -> ExitCode {
    let key = arguments
        .first()
        .map(String::as_str)
        .unwrap_or("default")
        .to_owned();
    if let Ok(text) = fs::read_to_string(root.join(format!("{key}.stdout"))) {
        print!("{text}");
    }
    let _ = io::stdout().flush();
    if let Ok(text) = fs::read_to_string(root.join(format!("{key}.stderr"))) {
        eprint!("{text}");
    }
    let code = fs::read_to_string(root.join(format!("{key}.exit")))
        .ok()
        .and_then(|text| text.trim().parse::<u8>().ok())
        .unwrap_or(0);
    if code == 0
        && key == "resume"
        && let Ok(state) = fs::read_to_string(root.join("resume.state.stdout"))
    {
        let _ = fs::write(root.join("status.stdout"), state);
    }
    ExitCode::from(code)
}
