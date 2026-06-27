//! CLI argument parsing.

use super::commands::*;

fn usage() -> &'static str {
    "Usage:\n  magiclaw                Start daemon mode\n  magiclaw --mcp          Start MCP server mode\n  magiclaw send --message <text> [--channel <wechat|feishu>] [--to <recipient>] [--receive-id-type <type>] [--context-token <token>] [--data-dir <wechat-dir>]\n  magiclaw auth issue --project <project_id> --name <client_name> --scopes send,window_status --ttl-secs <secs>\n  magiclaw auth list [--project <project_id>]\n  magiclaw auth revoke --token <raw_token>\n  magiclaw wechat login [--data-dir <dir>] [--account-id <id>]\n  magiclaw bind import (--jsonl <path> | --csv <path>)\n  magiclaw push import (--jsonl <path> | --csv <path>)\n  magiclaw push run --job <job_id>\n  magiclaw project list\n  magiclaw binding list --project <project_key>\n\nChannels:\n  --channel wechat (default)  Send via WeChat (ilink). Recipient inferred from context tokens if omitted.\n  --channel feishu            Send via Feishu OpenAPI. --to is required (open_id/chat_id/...).\n                              receive_id_type auto-detected from prefix (oc_->chat_id, ou_->open_id) or via --receive-id-type.\n\nEnvironment:\n  MAGICLAW_DB_PATH      SQLite database path shared by daemon and auth commands\n  WECHAT_CHANNEL_DIR    Default WeChat data directory (fallback: ~/.claude/channels/wechat)\n  MAGICLAW_API_TOKEN    Optional bearer token for localhost daemon /api/send and /api/window_status\n  MAGICLAW_API_SEND_DEBUG  Set to 1/true to include diagnostics in /api/send responses\n  FEISHU_*              Feishu channel config (APP_ID, APP_SECRET, RECEIVE_ID_TYPE, ...)"
}

pub fn parse_cli_args(args: &[String]) -> Result<CliCommand, String> {
    if args.len() <= 1 {
        return Ok(CliCommand::Daemon);
    }

    if args[1] == "--mcp" || args[1] == "mcp" {
        if args.len() != 2 {
            return Err(format!("--mcp does not accept extra arguments\n{}", usage()));
        }
        return Ok(CliCommand::Mcp);
    }

    if args[1] == "bind" {
        return parse_bind_args(&args[2..]);
    }

    if args[1] == "push" {
        return parse_push_args(&args[2..]);
    }

    if args[1] == "auth" {
        return parse_auth_args(&args[2..]).map(CliCommand::Auth);
    }

    if args[1] == "project" {
        if args.get(2).map(String::as_str) == Some("list") && args.len() == 3 {
            return Ok(CliCommand::ProjectList);
        }
        return Err(format!("usage: magiclaw project list\n{}", usage()));
    }

    if args[1] == "binding" {
        return parse_binding_args(&args[2..]);
    }

    if args[1] == "wechat" {
        return parse_wechat_args(&args[2..]).map(CliCommand::WeChat);
    }

    if args[1] != "send" {
        return Err(format!("unknown command: {}\n{}", args[1], usage()));
    }

    parse_send_args(&args[2..])
}

fn parse_scopes(value: &str) -> Result<Vec<String>, String> {
    let scopes: Vec<String> = value
        .split(',')
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect();
    if scopes.is_empty() {
        return Err("scopes cannot be empty".into());
    }
    Ok(scopes)
}

fn parse_send_args(args: &[String]) -> Result<CliCommand, String> {
    let mut data_dir = None::<String>;
    let mut to: Option<String> = None;
    let mut context_token: Option<String> = None;
    let mut message: Option<String> = None;
    let mut channel: Option<SendChannel> = None;
    let mut receive_id_type: Option<String> = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--channel" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("missing value for --channel\n{}", usage()))?;
                channel = Some(
                    SendChannel::parse(value).map_err(|e| format!("{}\n{}", e, usage()))?,
                );
            }
            "--receive-id-type" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    format!("missing value for --receive-id-type\n{}", usage())
                })?;
                receive_id_type = Some(value.clone());
            }
            "--data-dir" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("missing value for --data-dir\n{}", usage()))?;
                data_dir = Some(value.clone());
            }
            "--to" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("missing value for --to\n{}", usage()))?;
                to = Some(value.clone());
            }
            "--message" | "--text" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("missing value for --message\n{}", usage()))?;
                message = Some(value.clone());
            }
            "--context-token" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    format!("missing value for --context-token\n{}", usage())
                })?;
                context_token = Some(value.clone());
            }
            "--help" | "-h" => {
                return Err(usage().to_string());
            }
            other => {
                return Err(format!("unknown flag: {}\n{}", other, usage()));
            }
        }
        index += 1;
    }

    let message =
        message.ok_or_else(|| format!("missing required flag: --message\n{}", usage()))?;

    Ok(CliCommand::Send(SendCommand {
        channel: channel.unwrap_or(SendChannel::Wechat),
        data_dir: data_dir.unwrap_or_default(),
        to,
        context_token,
        receive_id_type,
        message,
    }))
}

fn parse_import_command(args: &[String]) -> Result<ImportCommand, String> {
    if args.first().map(String::as_str) != Some("import") {
        return Err(format!("expected `import` subcommand\n{}", usage()));
    }
    let rest = &args[1..];
    let mut format: Option<ImportFormat> = None;
    let mut path: Option<String> = None;
    let mut index = 0;
    while index < rest.len() {
        match rest[index].as_str() {
            "--jsonl" | "--csv" => {
                let fmt = if rest[index] == "--jsonl" {
                    ImportFormat::Jsonl
                } else {
                    ImportFormat::Csv
                };
                index += 1;
                let value = rest.get(index).ok_or_else(|| {
                    format!("missing path for {}\n{}", rest[index - 1], usage())
                })?;
                format = Some(fmt);
                path = Some(value.clone());
            }
            other => return Err(format!("unknown flag: {}\n{}", other, usage())),
        }
        index += 1;
    }
    let format = format.ok_or_else(|| format!("missing --jsonl or --csv\n{}", usage()))?;
    let path = path.ok_or_else(|| format!("missing import path\n{}", usage()))?;
    Ok(ImportCommand { format, path })
}

fn parse_bind_args(args: &[String]) -> Result<CliCommand, String> {
    parse_import_command(args).map(CliCommand::BindImport)
}

fn parse_push_args(args: &[String]) -> Result<CliCommand, String> {
    match args.first().map(String::as_str) {
        Some("import") => parse_import_command(args).map(CliCommand::PushImport),
        Some("run") => {
            let rest = &args[1..];
            if rest.len() == 2 && rest[0] == "--job" {
                Ok(CliCommand::PushRun(rest[1].clone()))
            } else {
                Err(format!(
                    "usage: magiclaw push run --job <job_id>\n{}",
                    usage()
                ))
            }
        }
        _ => Err(format!(
            "usage: magiclaw push (import ... | run --job <id>)\n{}",
            usage()
        )),
    }
}

fn parse_binding_args(args: &[String]) -> Result<CliCommand, String> {
    if args.len() == 3 && args[0] == "list" && args[1] == "--project" {
        return Ok(CliCommand::BindingList(args[2].clone()));
    }
    Err(format!(
        "usage: magiclaw binding list --project <project_key>\n{}",
        usage()
    ))
}

fn parse_auth_args(args: &[String]) -> Result<AuthCommand, String> {
    match args.first().map(String::as_str) {
        Some("issue") => {
            let mut project_id = None::<String>;
            let mut client_name = None::<String>;
            let mut scopes = None::<Vec<String>>;
            let mut ttl_secs = 86_400_i64;
            let mut index = 1;
            while index < args.len() {
                match args[index].as_str() {
                    "--project" => {
                        index += 1;
                        let value = args.get(index).ok_or_else(|| {
                            format!("missing value for --project\n{}", usage())
                        })?;
                        project_id = Some(value.clone());
                    }
                    "--name" => {
                        index += 1;
                        let value = args.get(index).ok_or_else(|| {
                            format!("missing value for --name\n{}", usage())
                        })?;
                        client_name = Some(value.clone());
                    }
                    "--scopes" => {
                        index += 1;
                        let value = args.get(index).ok_or_else(|| {
                            format!("missing value for --scopes\n{}", usage())
                        })?;
                        scopes = Some(parse_scopes(value)?);
                    }
                    "--ttl-secs" => {
                        index += 1;
                        let value = args.get(index).ok_or_else(|| {
                            format!("missing value for --ttl-secs\n{}", usage())
                        })?;
                        ttl_secs = value
                            .parse::<i64>()
                            .map_err(|e| format!("invalid --ttl-secs value: {}", e))?;
                    }
                    "--help" | "-h" => return Err(usage().to_string()),
                    other => return Err(format!("unknown flag: {}\n{}", other, usage())),
                }
                index += 1;
            }
            Ok(AuthCommand::Issue(AuthIssueCommand {
                project_id: project_id
                    .ok_or_else(|| format!("missing required flag: --project\n{}", usage()))?,
                client_name: client_name
                    .ok_or_else(|| format!("missing required flag: --name\n{}", usage()))?,
                scopes: scopes
                    .ok_or_else(|| format!("missing required flag: --scopes\n{}", usage()))?,
                ttl_secs,
            }))
        }
        Some("list") => {
            let mut project_id = None::<String>;
            let mut index = 1;
            while index < args.len() {
                match args[index].as_str() {
                    "--project" => {
                        index += 1;
                        let value = args.get(index).ok_or_else(|| {
                            format!("missing value for --project\n{}", usage())
                        })?;
                        project_id = Some(value.clone());
                    }
                    "--help" | "-h" => return Err(usage().to_string()),
                    other => return Err(format!("unknown flag: {}\n{}", other, usage())),
                }
                index += 1;
            }
            Ok(AuthCommand::List(AuthListCommand { project_id }))
        }
        Some("revoke") => {
            let mut token = None::<String>;
            let mut index = 1;
            while index < args.len() {
                match args[index].as_str() {
                    "--token" => {
                        index += 1;
                        let value = args.get(index).ok_or_else(|| {
                            format!("missing value for --token\n{}", usage())
                        })?;
                        token = Some(value.clone());
                    }
                    "--help" | "-h" => return Err(usage().to_string()),
                    other => return Err(format!("unknown flag: {}\n{}", other, usage())),
                }
                index += 1;
            }
            Ok(AuthCommand::Revoke(AuthRevokeCommand {
                token: token
                    .ok_or_else(|| format!("missing required flag: --token\n{}", usage()))?,
            }))
        }
        _ => Err(format!(
            "usage: magiclaw auth (issue|list|revoke)\n{}",
            usage()
        )),
    }
}

fn parse_wechat_args(args: &[String]) -> Result<WechatCommand, String> {
    match args.first().map(String::as_str) {
        Some("login") => {
            let mut data_dir = None::<String>;
            let mut account_id = None::<String>;
            let mut index = 1;
            while index < args.len() {
                match args[index].as_str() {
                    "--data-dir" => {
                        index += 1;
                        let value = args.get(index).ok_or_else(|| {
                            format!("missing value for --data-dir\n{}", usage())
                        })?;
                        data_dir = Some(value.clone());
                    }
                    "--account-id" => {
                        index += 1;
                        let value = args.get(index).ok_or_else(|| {
                            format!("missing value for --account-id\n{}", usage())
                        })?;
                        account_id = Some(value.clone());
                    }
                    "--help" | "-h" => {
                        return Err(
                            "usage: magiclaw wechat login [--data-dir <dir>] [--account-id <id>]"
                                .into(),
                        )
                    }
                    other => return Err(format!("unknown flag: {}\n{}", other, usage())),
                }
                index += 1;
            }
            Ok(WechatCommand::Login(WechatLoginCommand {
                data_dir,
                account_id,
            }))
        }
        _ => Err(format!("usage: magiclaw wechat (login)\n{}", usage())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_send_command_with_all_flags() {
        let args = vec![
            "magiclaw".into(),
            "send".into(),
            "--data-dir".into(),
            "/tmp/wechat".into(),
            "--message".into(),
            "hello".into(),
        ];

        let parsed = parse_cli_args(&args).unwrap();
        assert_eq!(
            parsed,
            CliCommand::Send(SendCommand {
                channel: SendChannel::Wechat,
                data_dir: "/tmp/wechat".into(),
                to: None,
                context_token: None,
                receive_id_type: None,
                message: "hello".into(),
            })
        );
    }

    #[test]
    fn parse_send_command_feishu_channel() {
        let args = vec![
            "magiclaw".into(),
            "send".into(),
            "--channel".into(),
            "feishu".into(),
            "--to".into(),
            "oc_abc123".into(),
            "--message".into(),
            "hello feishu".into(),
        ];

        let parsed = parse_cli_args(&args).unwrap();
        assert_eq!(
            parsed,
            CliCommand::Send(SendCommand {
                channel: SendChannel::Feishu,
                data_dir: String::new(),
                to: Some("oc_abc123".into()),
                context_token: None,
                receive_id_type: None,
                message: "hello feishu".into(),
            })
        );
    }

    #[test]
    fn parse_send_command_feishu_with_receive_id_type() {
        let args = vec![
            "magiclaw".into(),
            "send".into(),
            "--channel".into(),
            "lark".into(),
            "--to".into(),
            "ou_xyz".into(),
            "--receive-id-type".into(),
            "open_id".into(),
            "--message".into(),
            "hi".into(),
        ];

        match parse_cli_args(&args).unwrap() {
            CliCommand::Send(cmd) => {
                assert_eq!(cmd.channel, SendChannel::Feishu);
                assert_eq!(cmd.to.as_deref(), Some("ou_xyz"));
                assert_eq!(cmd.receive_id_type.as_deref(), Some("open_id"));
            }
            other => panic!("unexpected parse result: {:?}", other),
        }
    }

    #[test]
    fn send_channel_parse_aliases() {
        assert_eq!(SendChannel::parse("wechat").unwrap(), SendChannel::Wechat);
        assert_eq!(SendChannel::parse("wx").unwrap(), SendChannel::Wechat);
        assert_eq!(SendChannel::parse("feishu").unwrap(), SendChannel::Feishu);
        assert_eq!(SendChannel::parse("lark").unwrap(), SendChannel::Feishu);
        assert!(SendChannel::parse("telegram").is_err());
    }

    #[test]
    fn parse_mcp_flag_mode() {
        let args = vec!["magiclaw".into(), "--mcp".into()];
        let parsed = parse_cli_args(&args).unwrap();
        assert_eq!(parsed, CliCommand::Mcp);
    }

    #[test]
    fn parse_auth_issue_command() {
        let args = vec![
            "magiclaw".into(),
            "auth".into(),
            "issue".into(),
            "--project".into(),
            "proj-a".into(),
            "--name".into(),
            "worker-a".into(),
            "--scopes".into(),
            "send,window_status".into(),
            "--ttl-secs".into(),
            "3600".into(),
        ];
        match parse_cli_args(&args).unwrap() {
            CliCommand::Auth(AuthCommand::Issue(cmd)) => {
                assert_eq!(cmd.project_id, "proj-a");
                assert_eq!(cmd.client_name, "worker-a");
                assert_eq!(
                    cmd.scopes,
                    vec!["send".to_string(), "window_status".to_string()]
                );
                assert_eq!(cmd.ttl_secs, 3600);
            }
            other => panic!("unexpected parse result: {:?}", other),
        }
    }
}
