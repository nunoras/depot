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
    write_describe_output(arguments);
    let key = arguments
        .first()
        .map(String::as_str)
        .unwrap_or("default")
        .to_owned();
    if key == "resume" {
        return respond_resume(root, arguments);
    }
    let mut suffix = match sequence_index(root, &key) {
        Some(index) => format!(".{index}"),
        None => String::new(),
    };
    if !suffix.is_empty() && !root.join(format!("{key}.stdout{suffix}")).exists() {
        suffix = String::new();
    }
    if let Ok(text) = fs::read_to_string(root.join(format!("{key}.stdout{suffix}"))) {
        print!("{text}");
    }
    let _ = io::stdout().flush();
    if let Ok(text) = fs::read_to_string(root.join(format!("{key}.stderr{suffix}"))) {
        eprint!("{text}");
    }
    let code = fs::read_to_string(root.join(format!("{key}.exit{suffix}")))
        .ok()
        .and_then(|text| text.trim().parse::<u8>().ok())
        .unwrap_or(0);
    if code == 0
        && key == "resume"
        && let Ok(state) = fs::read_to_string(root.join("resume.state.stdout"))
    {
        let _ = fs::write(root.join("status.stdout"), state);
    }
    if code == 0
        && key == "stop"
        && let Some(session) = arguments.get(1)
    {
        let _ = fs::write(
            root.join("status.stdout"),
            format!("session: {session}\nstate: stopped\n"),
        );
    }
    ExitCode::from(code)
}

fn respond_resume(root: &Path, arguments: &[String]) -> ExitCode {
    let stderr = fs::read_to_string(root.join("resume.stderr")).unwrap_or_default();
    eprint!("{stderr}");
    let code = fs::read_to_string(root.join("resume.exit"))
        .ok()
        .and_then(|text| text.trim().parse::<u8>().ok())
        .unwrap_or(0);
    if code != 0 {
        if let Ok(text) = fs::read_to_string(root.join("resume.stdout")) {
            print!("{text}");
        }
        return ExitCode::from(code);
    }
    let parent = arguments
        .iter()
        .skip_while(|argument| *argument != "--detach")
        .nth(1)
        .cloned()
        .unwrap_or_default();
    let child = fs::read_to_string(root.join("resume.child"))
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| format!("{parent}-child"));
    print!("session: {child}\nresumedFrom: {parent}\n");
    let _ = io::stdout().flush();
    if let Ok(state) = fs::read_to_string(root.join("resume.state.stdout")) {
        let _ = fs::write(root.join("status.stdout"), state);
    }
    ExitCode::SUCCESS
}

fn write_describe_output(arguments: &[String]) {
    let describing = arguments
        .windows(2)
        .any(|pair| pair[0] == "--kind" && pair[1] == "describe");
    if !describing {
        return;
    }
    let Some(prompt) = arguments.last() else {
        return;
    };
    let Some(rest) = prompt.split_once("to the file ").map(|(_, rest)| rest) else {
        return;
    };
    let Some(path) = rest.lines().next().map(str::trim) else {
        return;
    };
    let _ = fs::write(
        path,
        "Describe the change\n\n## Why\n\nThe worker wrote this from the fetched base diff.\n",
    );
}

fn sequence_index(root: &Path, key: &str) -> Option<usize> {
    if !root.join(format!("{key}.stdout.1")).exists() {
        return None;
    }
    let calls = fs::read_to_string(root.join("calls.txt")).ok()?;
    Some(
        calls
            .split(RECORD)
            .filter(|record| record.split(SEPARATOR).next() == Some(key))
            .count(),
    )
}
