//! The command line. Small enough to parse by hand.

use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
};

pub const USAGE: &str = "\
Usage: frillogram [--config <path>] [--preview]
       frillogram --print-default-config
       frillogram --help

Options:
  --config <path>           Read the config file at <path>, which must exist
  --preview                 Show sample chats with the config's look, without Telegram:
                            no credentials, no login, nothing sent or stored
  --print-default-config    Print the example config file, with every key at its default
  -h, --help                Print this help

Without --config, the config file is $FRILLOGRAM_CONFIG, else
$XDG_CONFIG_HOME/frillogram/config.toml, else ~/.config/frillogram/config.toml.
SETUP.md describes the keys and the environment variables that override them.";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Start the app, with the config file `--config` named, if any, and
    /// with sample chats instead of Telegram for `--preview`.
    Run {
        config: Option<PathBuf>,
        preview: bool,
    },
    PrintDefaultConfig,
    Help,
}

/// Parses the arguments after the program name.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let mut config = None;
    let mut preview = false;
    while let Some(arg) = args.next() {
        let path = match arg.to_str() {
            Some("-h" | "--help") => return Ok(Command::Help),
            Some("--print-default-config") => return Ok(Command::PrintDefaultConfig),
            Some("--preview") => {
                preview = true;
                continue;
            }
            Some("--config") => args.next(),
            _ => match inline_config(&arg) {
                Some(path) => Some(path),
                None => return Err(format!("unknown argument {:?}", arg.to_string_lossy())),
            },
        };
        let path = path.filter(|path| !path.is_empty()).ok_or("--config needs a path")?;
        if config.replace(PathBuf::from(path)).is_some() {
            return Err("--config is given twice".into());
        }
    }
    Ok(Command::Run { config, preview })
}

/// The path of a `--config=<path>` argument. On Unix a path is any bytes, not
/// necessarily UTF-8, so the prefix is stripped from the raw bytes there.
fn inline_config(arg: &OsStr) -> Option<OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        arg.as_bytes().strip_prefix(b"--config=").map(|path| OsStr::from_bytes(path).to_owned())
    }
    #[cfg(not(unix))]
    {
        arg.to_str()?.strip_prefix("--config=").map(OsString::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strs(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn no_arguments_run_with_the_default_config() {
        assert_eq!(parse_strs(&[]), Ok(Command::Run { config: None, preview: false }));
    }

    #[test]
    fn preview_goes_with_or_without_a_config() {
        assert_eq!(parse_strs(&["--preview"]), Ok(Command::Run { config: None, preview: true }));
        let run = Ok(Command::Run { config: Some("a.toml".into()), preview: true });
        assert_eq!(parse_strs(&["--preview", "--config", "a.toml"]), run);
        assert_eq!(parse_strs(&["--config=a.toml", "--preview"]), run);
    }

    #[test]
    fn config_takes_a_path() {
        let run = Ok(Command::Run { config: Some("/x/config.toml".into()), preview: false });
        assert_eq!(parse_strs(&["--config", "/x/config.toml"]), run);
        assert_eq!(parse_strs(&["--config=/x/config.toml"]), run);
        assert_eq!(parse_strs(&["--config"]), Err("--config needs a path".into()));
        assert_eq!(parse_strs(&["--config="]), Err("--config needs a path".into()));
        assert_eq!(parse_strs(&["--config", "a", "--config", "b"]), Err("--config is given twice".into()));
    }

    #[cfg(unix)]
    #[test]
    fn config_paths_need_not_be_utf8() {
        use std::os::unix::ffi::OsStrExt;

        let path = OsStr::from_bytes(b"/x/\xffconfig.toml");
        let run = Ok(Command::Run { config: Some(path.into()), preview: false });
        assert_eq!(parse([OsString::from("--config"), path.to_owned()]), run);
        let mut inline = OsString::from("--config=");
        inline.push(path);
        assert_eq!(parse([inline]), run);
        let unknown = OsStr::from_bytes(b"--conf\xff").to_owned();
        assert_eq!(parse([unknown]), Err("unknown argument \"--conf\u{fffd}\"".into()));
    }

    #[test]
    fn help_and_print_default_config() {
        assert_eq!(parse_strs(&["--help"]), Ok(Command::Help));
        assert_eq!(parse_strs(&["-h"]), Ok(Command::Help));
        assert_eq!(parse_strs(&["--print-default-config"]), Ok(Command::PrintDefaultConfig));
        assert_eq!(parse_strs(&["--config", "a", "--print-default-config"]), Ok(Command::PrintDefaultConfig));
    }

    #[test]
    fn unknown_arguments_are_errors() {
        assert_eq!(parse_strs(&["--verbose"]), Err("unknown argument \"--verbose\"".into()));
        assert_eq!(parse_strs(&["config.toml"]), Err("unknown argument \"config.toml\"".into()));
        assert_eq!(parse_strs(&["--help", "--verbose"]), Ok(Command::Help));
    }
}
