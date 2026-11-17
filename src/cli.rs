//! Command line of the `astlens` binary.

use std::ffi::OsString;
use std::path::PathBuf;

pub const HELP: &str = "\
AST-aware code navigation MCP server

Usage: astlens <OPTION>

Options:
  -i, --install [--no-enforce] [--client CLIENT]
                                Set up Claude Code and/or OpenCode: MCP server,
                                plus hook/plugin and instructions when enforcing.
                                CLIENT is claude, opencode or all (the default).
                                --no-enforce registers the MCP server only
  -u, --uninstall               Undo --install, remove the PATH entry and this binary
  --update [--check]            Download the latest release over this binary, from
                                GitHub Releases after checking it against SHA256SUMS.
                                A binary that cargo installed is left alone and the
                                cargo command is printed instead; --check only reports
  -s, --serve [ROOT]            Run the MCP server over stdio; the client starts this itself.
                                ROOT defaults to $ASTLENS_ROOT, then the current directory
  -h, --help                    Show this help
  -v, --version                 Show the version
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Client {
    Claude,
    Opencode,
    All,
}

impl Client {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "claude" => Ok(Self::Claude),
            "opencode" => Ok(Self::Opencode),
            "all" => Ok(Self::All),
            _ => Err(format!(
                "unknown client: {value} (expected claude, opencode or all)"
            )),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    Serve(Option<PathBuf>),
    Install { enforce: bool, client: Client },
    Uninstall,
    Update { check_only: bool },
}

/// Parse the arguments after the program name. `interactive` tells whether
/// stdin is a terminal.
///
/// Older registrations run the binary with no argument, with a root directory,
/// or with an earlier `serve` command. Their stdin is a pipe, so outside a
/// terminal all three still start the server.
pub fn parse(args: &[OsString], interactive: bool) -> Result<Command, String> {
    let mut args = args.iter();
    let Some(first) = args.next() else {
        return Ok(if interactive {
            Command::Help
        } else {
            Command::Serve(None)
        });
    };
    let rest: Vec<&OsString> = args.collect();
    let no_extra = |command: Command| {
        if let Some(extra) = rest.first() {
            return Err(format!("unexpected argument: {}", extra.display()));
        }
        Ok(command)
    };

    match first.to_str() {
        Some("-h" | "--help") => no_extra(Command::Help),
        Some("-v" | "--version") => no_extra(Command::Version),
        Some("-u" | "--uninstall") => no_extra(Command::Uninstall),
        Some("-i" | "--install") => {
            let mut enforce = true;
            let mut client = Client::All;
            let mut i = 0;
            while i < rest.len() {
                let arg = rest[i].to_string_lossy();
                match arg.as_ref() {
                    "--no-enforce" => enforce = false,
                    "--client" => {
                        let Some(value) = rest.get(i + 1) else {
                            return Err("--client needs claude, opencode or all".into());
                        };
                        client = Client::parse(&value.to_string_lossy())?;
                        i += 1;
                    }
                    value if value.starts_with("--client=") => {
                        client = Client::parse(&value["--client=".len()..])?;
                    }
                    _ => return Err(format!("unexpected argument: {arg}")),
                }
                i += 1;
            }
            Ok(Command::Install { enforce, client })
        }
        Some("--update") => {
            let mut check_only = false;
            for arg in &rest {
                match arg.to_string_lossy().as_ref() {
                    "--check" | "--dry-run" => check_only = true,
                    value => return Err(format!("unexpected argument: {value}")),
                }
            }
            Ok(Command::Update { check_only })
        }
        Some("-s" | "--serve" | "serve") => {
            // `serve` is how an older registration spelled the flag: accepted
            // at a terminal too, with the same rest.
            match rest.as_slice() {
                [] => Ok(Command::Serve(None)),
                [root] => Ok(Command::Serve(Some(PathBuf::from(root)))),
                [_, extra, ..] => Err(format!("unexpected argument: {}", extra.display())),
            }
        }
        Some(flag) if flag.starts_with('-') => Err(format!("unknown option: {flag}")),
        _ if interactive => Err(format!("unknown command: {}", first.display())),
        _ => no_extra(Command::Serve(Some(PathBuf::from(first)))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str], interactive: bool) -> Result<Command, String> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        parse(&args, interactive)
    }

    #[test]
    fn no_argument_is_help_in_a_terminal_and_serve_in_a_pipe() {
        assert_eq!(run(&[], true), Ok(Command::Help));
        assert_eq!(run(&[], false), Ok(Command::Serve(None)));
    }

    #[test]
    fn commands_ignore_whether_stdin_is_a_terminal() {
        for interactive in [true, false] {
            for flag in ["--install", "-i"] {
                assert_eq!(
                    run(&[flag], interactive),
                    Ok(Command::Install {
                        enforce: true,
                        client: Client::All
                    })
                );
                assert_eq!(
                    run(&[flag, "--no-enforce"], interactive),
                    Ok(Command::Install {
                        enforce: false,
                        client: Client::All
                    })
                );
                assert_eq!(
                    run(&[flag, "--client", "opencode"], interactive),
                    Ok(Command::Install {
                        enforce: true,
                        client: Client::Opencode
                    })
                );
                assert_eq!(
                    run(&[flag, "--client=claude", "--no-enforce"], interactive),
                    Ok(Command::Install {
                        enforce: false,
                        client: Client::Claude
                    })
                );
            }
            for flag in ["--uninstall", "-u"] {
                assert_eq!(run(&[flag], interactive), Ok(Command::Uninstall));
            }
            assert_eq!(
                run(&["--update"], interactive),
                Ok(Command::Update { check_only: false })
            );
            for flag in ["--check", "--dry-run"] {
                assert_eq!(
                    run(&["--update", flag], interactive),
                    Ok(Command::Update { check_only: true })
                );
            }
            for flag in ["--serve", "-s"] {
                assert_eq!(run(&[flag], interactive), Ok(Command::Serve(None)));
                assert_eq!(
                    run(&[flag, "/p"], interactive),
                    Ok(Command::Serve(Some("/p".into())))
                );
            }
            for flag in ["--help", "-h"] {
                assert_eq!(run(&[flag], interactive), Ok(Command::Help));
            }
            for flag in ["--version", "-v"] {
                assert_eq!(run(&[flag], interactive), Ok(Command::Version));
            }
        }
    }

    #[test]
    fn the_old_serve_command_works_at_a_terminal_too() {
        assert_eq!(run(&["serve"], false), Ok(Command::Serve(None)));
        assert_eq!(
            run(&["serve", "/p"], false),
            Ok(Command::Serve(Some("/p".into())))
        );
        // A word the help already shows as the flag's spelling is no typo to
        // refuse: only a path stays unserved there.
        assert_eq!(run(&["serve"], true), Ok(Command::Serve(None)));
        assert_eq!(
            run(&["serve", "/p"], true),
            Ok(Command::Serve(Some("/p".into())))
        );
    }

    #[test]
    fn old_command_words_are_rejected_in_a_terminal() {
        for word in ["install", "uninstall", "help", "version"] {
            assert!(run(&[word], true).is_err());
        }
    }

    #[test]
    fn update_is_a_flag_only() {
        assert_eq!(
            run(&["--update"], true),
            Ok(Command::Update { check_only: false })
        );
        // Without the flag, the word is an unexpected command in a terminal and
        // a workspace root in a pipe, like any other word.
        assert!(run(&["update"], true).is_err());
        assert_eq!(
            run(&["update"], false),
            Ok(Command::Serve(Some("update".into())))
        );
    }

    #[test]
    fn a_bare_path_is_a_root_only_outside_a_terminal() {
        assert_eq!(run(&["/p"], false), Ok(Command::Serve(Some("/p".into()))));
        assert!(run(&["/p"], true).is_err());
    }

    #[test]
    fn bad_arguments_are_rejected() {
        assert!(run(&["--nope"], false).is_err());
        assert!(run(&["--install", "--nope"], true).is_err());
        assert!(run(&["--install", "--client"], true).is_err());
        assert!(run(&["--install", "--client", "vim"], true).is_err());
        assert!(run(&["--install", "--client="], true).is_err());
        assert!(run(&["--uninstall", "x"], true).is_err());
        assert!(run(&["--update", "x"], true).is_err());
        assert!(run(&["--update", "--check", "x"], false).is_err());
        assert!(run(&["--serve", "a", "b"], true).is_err());
        assert!(run(&["/p", "x"], false).is_err());
        assert!(run(&["serve", "a", "b"], true).is_err());
    }
}
