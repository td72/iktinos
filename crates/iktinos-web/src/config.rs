//! The configuration file: `~/.config/iktinos/config.toml`, or the one
//! `--config` names. It is the terminal viewer's file, and the server reads
//! the part of it that means something to a server:
//!
//! ```toml
//! projects_dir = "~/.claude/projects"
//! cwd = false
//! refresh = "1s"
//!
//! [live]
//! enabled = true
//! socket_dir = "~/sockets"
//!
//! [sessions]
//! active_within = "1m"
//!
//! [tools]
//! recent = 200
//! output_chars = 2000
//! ```
//!
//! An argument goes before the file, the file before the defaults
//! ([`Config::new`]). The file is only ever read.
//!
//! The rest of the file is the viewer's (its pages, its colours) and is
//! passed over, so a key the server does not know is not an error here,
//! and a misspelt one goes unnoticed. A key it does know with a value it
//! cannot read stops the server at startup with the line it is on.
//!
//! Nothing here is shared with the viewer's own reading of the file
//! (`iktinos-tui`): the two crates do not depend on each other.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::{self, Deserializer};
use serde::Deserialize;
use toml::Spanned;

use crate::{Config, DEFAULT_PORT};

/// What the command line asked for. `None` and `false` leave the choice to
/// the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    /// `--config`: the file to read instead of the default one.
    pub config: Option<PathBuf>,
    /// `--port`. No file sets it.
    pub port: Option<u16>,
    pub projects_dir: Option<PathBuf>,
    pub socket_dir: Option<PathBuf>,
    pub no_live: bool,
    pub cwd: bool,
}

/// What the server cannot decide from its arguments and its file.
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// For a leading `~` in a path.
    pub home: Option<PathBuf>,
    /// For `--cwd`.
    pub current_dir: Option<PathBuf>,
    /// Where Claude Code keeps its transcripts, unless told otherwise.
    pub default_projects_dir: Option<PathBuf>,
}

/// The part of the file the server reads. Everything may be left out, and
/// what else the file holds is the viewer's.
#[derive(Debug, Default, Deserialize)]
pub struct File {
    projects_dir: Option<PathBuf>,
    cwd: Option<bool>,
    refresh: Option<Spanned<Length>>,
    #[serde(default)]
    live: Live,
    #[serde(default)]
    sessions: Sessions,
    #[serde(default)]
    tools: Tools,
}

#[derive(Debug, Default, Deserialize)]
struct Live {
    enabled: Option<bool>,
    socket_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct Sessions {
    active_within: Option<Length>,
}

#[derive(Debug, Default, Deserialize)]
struct Tools {
    recent: Option<usize>,
    output_chars: Option<usize>,
}

/// A length of time: `90s`, `30m`, `1h`, `2d`, or a number of seconds.
#[derive(Debug, Clone, Copy)]
struct Length(Duration);

impl Length {
    fn parse(text: &str) -> Option<Duration> {
        let (number, unit) = match text.char_indices().last()? {
            (at, 's') => (&text[..at], 1),
            (at, 'm') => (&text[..at], 60),
            (at, 'h') => (&text[..at], 3_600),
            (at, 'd') => (&text[..at], 86_400),
            _ => (text, 1),
        };
        let seconds = number.parse::<u64>().ok()?.checked_mul(unit)?;
        Some(Duration::from_secs(seconds))
    }
}

impl<'de> Deserialize<'de> for Length {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl de::Visitor<'_> for Visitor {
            type Value = Length;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a length of time such as \"90s\", \"30m\" or \"1h\"")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<Length, E> {
                Length::parse(text)
                    .map(Length)
                    .ok_or_else(|| E::invalid_value(de::Unexpected::Str(text), &self))
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}

/// Where the file is unless `--config` names one:
/// `$XDG_CONFIG_HOME/iktinos/config.toml`, else
/// `~/.config/iktinos/config.toml`. `None` without either directory.
pub fn default_path(xdg_config_home: Option<OsString>, home: Option<&Path>) -> Option<PathBuf> {
    let dir = xdg_config_home
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".config")))?;
    Some(dir.join("iktinos").join("config.toml"))
}

/// Reads the file at `path`. One that is not there is an error only if it
/// was asked for by name (`named`); otherwise every setting is left to its
/// default.
pub fn load(path: &Path, named: bool) -> Result<File, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !named => Ok(File::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Reads the text of a file, and checks what the types alone do not.
pub fn parse(text: &str) -> Result<File, String> {
    let file: File = toml::from_str(text).map_err(|e| e.to_string().trim_end().to_owned())?;
    if let Some(refresh) = &file.refresh {
        if refresh.get_ref().0.is_zero() {
            let at = refresh.span().start.min(text.len());
            return Err(format!(
                "line {}: refresh must be longer than nothing",
                text[..at].matches('\n').count() + 1
            ));
        }
    }
    Ok(file)
}

/// The settings the server runs with: an argument where one was given,
/// else the file's, else the default.
pub fn resolve(args: Args, file: File, env: &Env) -> Result<Config, String> {
    let home = |path: PathBuf| match (path.strip_prefix("~"), &env.home) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path,
    };
    let projects_dir = args
        .projects_dir
        .or(file.projects_dir.map(home))
        .or_else(|| env.default_projects_dir.clone())
        .ok_or("no home directory; pass --projects-dir")?;
    let mut config = Config::new(projects_dir);
    config.port = args.port.unwrap_or(DEFAULT_PORT);

    if args.cwd || file.cwd == Some(true) {
        let dir = env
            .current_dir
            .clone()
            .ok_or("--cwd: cannot tell the current directory")?;
        config.within = Some(dir);
    }
    if let Some(refresh) = file.refresh {
        config.refresh = refresh.into_inner().0;
    }

    config.live = !args.no_live && file.live.enabled.unwrap_or(true);
    config.socket_dir = args.socket_dir.or(file.live.socket_dir.map(home));

    if let Some(active) = file.sessions.active_within {
        config.active_within = active.0;
    }
    if let Some(recent) = file.tools.recent {
        config.recent_tools = recent;
    }
    if let Some(chars) = file.tools.output_chars {
        config.output_chars = chars;
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env {
            home: Some("/synthetic/home".into()),
            current_dir: Some("/synthetic/work".into()),
            default_projects_dir: Some("/synthetic/home/.claude/projects".into()),
        }
    }

    fn config(text: &str, args: Args) -> Config {
        resolve(args, parse(text).unwrap(), &env()).unwrap()
    }

    #[test]
    fn without_a_file_everything_is_its_default() {
        assert_eq!(
            config("", Args::default()),
            Config::new("/synthetic/home/.claude/projects".into())
        );
    }

    #[test]
    fn every_key_the_server_knows_is_read() {
        let read = config(
            r#"
projects_dir = "~/elsewhere"
cwd = true
refresh = "5s"

[live]
enabled = false
socket_dir = "~/sockets"

[sessions]
active_within = "2m"

[tools]
recent = 50
output_chars = 0
"#,
            Args::default(),
        );
        assert_eq!(
            read,
            Config {
                projects_dir: "/synthetic/home/elsewhere".into(),
                port: DEFAULT_PORT,
                live: false,
                socket_dir: Some("/synthetic/home/sockets".into()),
                within: Some("/synthetic/work".into()),
                refresh: Duration::from_secs(5),
                active_within: Duration::from_secs(120),
                recent_tools: 50,
                output_chars: 0,
            }
        );
    }

    #[test]
    fn an_argument_goes_before_the_file() {
        let read = config(
            r#"
projects_dir = "~/elsewhere"

[live]
socket_dir = "~/sockets"
"#,
            Args {
                port: Some(0),
                projects_dir: Some("/synthetic/named".into()),
                socket_dir: Some("/synthetic/named-sockets".into()),
                no_live: true,
                cwd: true,
                ..Args::default()
            },
        );
        assert_eq!(read.projects_dir, PathBuf::from("/synthetic/named"));
        assert_eq!(read.port, 0);
        assert_eq!(
            read.socket_dir,
            Some(PathBuf::from("/synthetic/named-sockets"))
        );
        assert!(!read.live);
        assert_eq!(read.within, Some(PathBuf::from("/synthetic/work")));
    }

    #[test]
    fn what_is_the_viewers_is_passed_over() {
        let read = config(
            r##"
refresh = "2s"

[pages]
order = ["sessions", "trace"]

[time]
style = "relative"

[live]
quiet_after = "3m"

[sessions]
sort = "cost"

[trace]
slow = ["10s", "1m"]

[colors]
error = "#ff5555"
"##,
            Args::default(),
        );
        assert_eq!(
            read,
            Config {
                refresh: Duration::from_secs(2),
                ..Config::new("/synthetic/home/.claude/projects".into())
            }
        );
    }

    #[test]
    fn what_cannot_be_read_is_an_error_that_names_its_line() {
        for (text, line, about) in [
            ("refresh = \"soon\"", 1, "a length of time"),
            ("\nrefresh = \"0s\"", 2, "longer than nothing"),
            ("cwd = \"yes\"", 1, "boolean"),
            ("[tools]\nrecent = \"many\"", 2, "recent"),
            ("[sessions]\n\nactive_within = 60", 3, "a length of time"),
            ("[live\nenabled = true", 1, ""),
        ] {
            let error = parse(text).unwrap_err();
            assert!(error.contains(&format!("line {line}")), "{text:?}: {error}");
            assert!(error.contains(about), "{text:?}: {error}");
        }
    }

    #[test]
    fn a_file_that_is_not_there_is_an_error_only_when_named() {
        let missing = Path::new("/synthetic/nowhere/config.toml");
        assert!(load(missing, false).is_ok());
        let error = load(missing, true).unwrap_err();
        assert!(error.starts_with("/synthetic/nowhere/config.toml: "));
    }

    #[test]
    fn the_file_is_under_the_configuration_directory() {
        let home = Path::new("/synthetic/home");
        assert_eq!(
            default_path(None, Some(home)),
            Some("/synthetic/home/.config/iktinos/config.toml".into())
        );
        assert_eq!(
            default_path(Some("/synthetic/xdg".into()), Some(home)),
            Some("/synthetic/xdg/iktinos/config.toml".into())
        );
        assert_eq!(
            default_path(Some(OsString::new()), Some(home)),
            Some("/synthetic/home/.config/iktinos/config.toml".into())
        );
        assert_eq!(default_path(None, None), None);
    }

    #[test]
    fn without_a_home_the_projects_directory_must_be_named() {
        let nowhere = Env::default();
        assert!(resolve(Args::default(), File::default(), &nowhere).is_err());
        let named = Args {
            projects_dir: Some("/synthetic/named".into()),
            ..Args::default()
        };
        assert!(resolve(named, File::default(), &nowhere).is_ok());
    }
}
