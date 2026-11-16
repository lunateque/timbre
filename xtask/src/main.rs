//! The repository gate: `cargo xtask <command>`.
//!
//! Every check reads the tracked state of the repository and the cargo metadata, and
//! reports violations instead of fixing them; the commands live in submodules.

mod check;
mod commits;
mod lexer;
mod module;
mod process;
mod removability;
mod report;
mod workspace;

use std::process::ExitCode;

/// The error of a whole command, rendered as `error: <message>` on stderr.
///
/// xtask is not a module: it talks to a terminal, not to an HTTP client, so a plain
/// message beats the `Failure` trait.
pub(crate) type Outcome<T = ()> = Result<T, String>;

const USAGE: &str = "\
cargo xtask check
cargo xtask commits <revision-range>
cargo xtask commits --message-file <path>
cargo xtask module new <id>
cargo xtask module remove <id>
cargo xtask removability";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let outcome: Outcome<Vec<String>> = match args.as_slice() {
        [one] if one == "check" => check::run().map(|violations| {
            violations
                .iter()
                .map(check::Violation::render)
                .collect::<Vec<String>>()
        }),
        [one] if one == "removability" => removability::run().map(|()| Vec::new()),
        [one, range] if one == "commits" => commits::in_range(range),
        [one, flag, path] if one == "commits" && flag == "--message-file" => {
            commits::message_file(path)
        }
        [one, verb, id] if one == "module" && verb == "new" => module::new(id).map(|()| Vec::new()),
        [one, verb, id] if one == "module" && verb == "remove" => {
            module::remove(id).map(|()| Vec::new())
        }
        _ => {
            for line in USAGE.lines() {
                report::line(line);
            }
            return ExitCode::from(2);
        }
    };
    match outcome {
        Err(message) => {
            report::line(&format!("error: {message}"));
            ExitCode::FAILURE
        }
        Ok(problems) => {
            for problem in &problems {
                report::line(problem);
            }
            if problems.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
    }
}
