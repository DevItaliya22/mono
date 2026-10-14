// SPDX-License-Identifier: MPL-2.0

//! Koru AI Chat's deliberately small command line.
//!
//! This companion is machine-wide. Accepting Koru's profile/workspace flags
//! would imply that it owns content startup, and was the path that accidentally
//! sent a fresh chat launch through the profile picker.

use std::path::PathBuf;

pub type ChatLink = (koru_ai_core::ChatId, Option<koru_ai_core::MessageBlockId>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Run,
    Stop,
    Help,
    Version,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub action: Action,
    pub root: Option<PathBuf>,
    pub link: Option<ChatLink>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejected(pub String);

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Invocation, Rejected> {
    let args: Vec<String> = args.into_iter().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(Invocation {
            action: Action::Help,
            root: None,
            link: None,
        });
    }
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        return Ok(Invocation {
            action: Action::Version,
            root: None,
            link: None,
        });
    }

    let mut action = Action::Run;
    let mut root = None;
    let mut link = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "stop" {
            if action == Action::Stop {
                return Err(Rejected("`stop` was supplied more than once".into()));
            }
            action = Action::Stop;
            i += 1;
            continue;
        }
        let (name, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(name, value)| (name, Some(value)));
        let value = match name {
            "--root" | "--chat-link" => match inline {
                Some("") => return Err(Rejected(format!("`{name}` needs a value"))),
                Some(value) => value.to_owned(),
                None => {
                    let Some(value) = args.get(i + 1).filter(|value| !value.starts_with('-'))
                    else {
                        return Err(Rejected(format!("`{name}` needs a value")));
                    };
                    i += 1;
                    value.clone()
                }
            },
            _ if arg.starts_with('-') => {
                return Err(Rejected(format!("unrecognised option `{arg}`")));
            }
            _ => return Err(Rejected(format!("unexpected argument `{arg}`"))),
        };
        match name {
            "--root" => root = Some(PathBuf::from(value)),
            "--chat-link" => {
                let koru_link::KoruLink::Chat { chat, block } = koru_link::parse(&value)
                    .map_err(|_| Rejected("`--chat-link` needs a koru://chat link".into()))?
                else {
                    return Err(Rejected("`--chat-link` needs a koru://chat link".into()));
                };
                link = Some((
                    koru_ai_core::ChatId::from_uuid(chat),
                    block.map(koru_ai_core::MessageBlockId::from_uuid),
                ));
            }
            _ => unreachable!(),
        }
        i += 1;
    }
    if action == Action::Stop && link.is_some() {
        return Err(Rejected(
            "`stop` cannot be combined with `--chat-link`".into(),
        ));
    }
    Ok(Invocation { action, root, link })
}

pub fn version() -> &'static str {
    option_env!("KORU_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

pub fn help(program: &str) -> String {
    format!(
        "Koru AI {} - standalone AI chat\n\n\
         Usage: {program} [options]\n\
                {program} stop [--root <path>]\n\n\
           --root <path>       Use this channel root for Agent and Store data.\n\
           --chat-link <url>   Open an exact koru://chat/<id>[#<block>] link.\n\
           --version, -V       Print the version and exit.\n\
           --help, -h          Show this message.\n\n\
         Koru AI Chat is machine-wide and does not select a profile or workspace.\n",
        version()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        std::iter::once("koru-ai-chat".to_owned())
            .chain(args.iter().map(|arg| (*arg).to_owned()))
            .collect()
    }

    #[test]
    fn ordinary_and_rooted_runs_are_profile_free() {
        assert_eq!(
            parse(argv(&[])),
            Ok(Invocation {
                action: Action::Run,
                root: None,
                link: None,
            })
        );
        assert_eq!(
            parse(argv(&["--root", "C:/isolated"])).unwrap().root,
            Some(PathBuf::from("C:/isolated"))
        );
        for obsolete in ["--profile", "--workspace", "--pick-profile", "--profiles"] {
            assert!(
                parse(argv(&[obsolete])).is_err(),
                "{obsolete} must be refused"
            );
        }
    }

    #[test]
    fn exact_chat_links_and_equals_values_parse() {
        let chat = koru_ai_core::ChatId::new();
        let block = koru_ai_core::MessageBlockId::new();
        let link = koru_link::chat_link(chat.as_uuid(), Some(block.as_uuid()));
        let parsed = parse(argv(&[
            &format!("--root={}", "C:/isolated"),
            &format!("--chat-link={link}"),
        ]))
        .unwrap();
        assert_eq!(parsed.link, Some((chat, Some(block))));
    }

    #[test]
    fn one_shot_commands_win_without_starting_the_runtime() {
        assert_eq!(
            parse(argv(&["--help", "--nonsense"])).unwrap().action,
            Action::Help
        );
        assert_eq!(parse(argv(&["--version"])).unwrap().action, Action::Version);
        assert_eq!(
            parse(argv(&["stop", "--root", "C:/isolated"]))
                .unwrap()
                .action,
            Action::Stop
        );
    }

    #[test]
    fn malformed_or_wrong_kind_links_are_refused() {
        assert!(parse(argv(&["--chat-link", "not-a-link"])).is_err());
        assert!(parse(argv(&["--chat-link", "koru://document/nope"])).is_err());
    }
}
