#![allow(clippy::or_fun_call)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;

use anyhow::{anyhow, Context as _};
use chrono::prelude::*;
use json_color::{Color, Colorizer};

use radicle::identity::project::{Doc, Untrusted};
use radicle::identity::Id;
use radicle::storage::{ReadRepository, ReadStorage, WriteStorage};

use crate::terminal as term;
use crate::terminal::args::{Args, Error, Help};

pub const HELP: Help = Help {
    name: "inspect",
    description: "Inspect a radicle identity or project directory",
    version: env!("CARGO_PKG_VERSION"),
    usage: r#"
Usage

    rad inspect <path> [<option>...]
    rad inspect <id>   [<option>...]
    rad inspect

    Inspects the given path or ID. If neither is specified,
    the current project is inspected.

Options

    --id        Return the ID in simplified form
    --payload   Inspect the object's payload
    --refs      Inspect the object's refs on the local device (requires `tree`)
    --history   Show object's history
    --help      Print help
"#,
};

#[derive(Default, Debug, Eq, PartialEq)]
pub struct Options {
    pub path: Option<PathBuf>,
    pub id: Option<Id>,
    pub refs: bool,
    pub payload: bool,
    pub history: bool,
    pub id_only: bool,
}

impl Args for Options {
    fn from_args(args: Vec<OsString>) -> anyhow::Result<(Self, Vec<OsString>)> {
        use lexopt::prelude::*;

        let mut parser = lexopt::Parser::from_args(args);
        let mut path: Option<PathBuf> = None;
        let mut id: Option<Id> = None;
        let mut refs = false;
        let mut payload = false;
        let mut history = false;
        let mut id_only = false;

        while let Some(arg) = parser.next()? {
            match arg {
                Long("help") => {
                    return Err(Error::Help.into());
                }
                Long("refs") => {
                    refs = true;
                }
                Long("payload") => {
                    payload = true;
                }
                Long("history") => {
                    history = true;
                }
                Long("id") => {
                    id_only = true;
                }
                Value(val) if path.is_none() && id.is_none() => {
                    let val = val.to_string_lossy();

                    if let Ok(val) = Id::from_str(&val) {
                        id = Some(val);
                    } else if let Ok(val) = PathBuf::from_str(&val) {
                        path = Some(val);
                    } else {
                        return Err(anyhow!("invalid Path or ID '{}'", val));
                    }
                }
                _ => return Err(anyhow::anyhow!(arg.unexpected())),
            }
        }

        Ok((
            Options {
                id,
                path,
                payload,
                history,
                refs,
                id_only,
            },
            vec![],
        ))
    }
}

pub fn run(options: Options, ctx: impl term::Context) -> anyhow::Result<()> {
    let profile = ctx.profile()?;
    let storage = &profile.storage;
    let signer = term::signer(&profile)?;

    let id = options
        .id
        .or_else(|| {
            radicle::rad::repo(options.path.unwrap_or_else(|| Path::new(".").to_path_buf()))
                .ok()
                .map(|(_, id)| id)
        })
        .context("Couldn't get ID / Path from command line and cwd is not a reposiitory either")?;

    let project = storage
        .get(signer.public_key(), id)?
        .context("No project with such ID exists")?;

    if options.refs {
        let path = profile
            .paths()
            .storage()
            .join(id.to_human())
            .join("refs")
            .join("namespaces");

        Command::new("tree")
            .current_dir(path)
            .args(["--noreport", "--prune"])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?
            .wait()?;
    } else if options.payload {
        println!(
            "{}",
            colorizer().colorize_json_str(&serde_json::to_string_pretty(&project.payload)?)?
        );
    } else if options.history {
        let repo = storage.repository(id)?;
        let head = Doc::<Untrusted>::head(signer.public_key(), &repo)?;
        let history = repo.revwalk(head)?.collect::<Vec<_>>();
        let revision = history.len() as usize;

        for (counter, oid) in history.into_iter().rev().enumerate() {
            let oid = oid?.into();
            let tip = repo.commit(oid)?;
            let blob = Doc::blob_at(oid, &repo)?;
            let content = String::from_utf8_lossy(blob.content());
            let content = content.replace("\"verified\":null,", "");
            let content = content.replace(",\"name\":\"anonymous\"", "");
            let content: serde_json::Value = serde_json::from_slice(content.as_bytes())?;
            let timezone = if tip.time().sign() == '+' {
                FixedOffset::east(tip.time().offset_minutes() * 60)
            } else {
                FixedOffset::west(tip.time().offset_minutes() * 60)
            };
            let time = DateTime::<Utc>::from(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(tip.time().seconds() as u64),
            )
            .with_timezone(&timezone)
            .to_rfc2822();

            print!(
                "{}",
                term::TextBox::new(format!(
                    "commit {}\nblob   {}\ndate   {}\n\n{}",
                    term::format::yellow(oid),
                    term::format::dim(blob.id()),
                    term::format::dim(time),
                    colorizer().colorize_json_str(&serde_json::to_string_pretty(&content)?)?,
                ))
                .first(counter == 0)
                .last(counter + 1 == revision)
            );
        }
    } else if options.id_only {
        term::info!("{}", term::format::highlight(id.to_human()));
    } else {
        term::info!("{}", term::format::highlight(id));
    }

    Ok(())
}

// Used for JSON Colorizing
fn colorizer() -> Colorizer {
    Colorizer::new()
        .null(Color::Cyan)
        .boolean(Color::Yellow)
        .number(Color::Magenta)
        .string(Color::Green)
        .key(Color::Blue)
        .build()
}
