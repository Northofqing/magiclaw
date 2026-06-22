use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fs2::FileExt;

use magiclaw::adapters::api_client_registry::ApiClientRegistry;
use magiclaw::adapters::mcp::server::McpServer;
use magiclaw::channels::wechat::ilink::{send_text_via_ilink, ILinkSendConfig};
use magiclaw::infrastructure::config::AppConfig;
use magiclaw::infrastructure::runtime::AppRuntime;
use magiclaw::infrastructure::tracing_init;
use serde::{Deserialize, Serialize};

/// Default HTTP API address for the magiclaw daemon (matches weclaw convention).
const DEFAULT_API_ADDR: &str = "127.0.0.1:18011";

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliCommand {
    Daemon,
    Mcp,
    Send(SendCommand),
    Auth(AuthCommand),
    BindImport(ImportCommand),
    PushImport(ImportCommand),
    PushRun(String),
    ProjectList,
    BindingList(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AuthCommand {
    Issue(AuthIssueCommand),
    List(AuthListCommand),
    Revoke(AuthRevokeCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthIssueCommand {
    project_id: String,
    client_name: String,
    scopes: Vec<String>,
    ttl_secs: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthListCommand {
    project_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthRevokeCommand {
    token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SendCommand {
    data_dir: String,
    to: Option<String>,
    message: String,
}

/// File-import command: exactly one of jsonl/csv is set.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ImportCommand {
    format: ImportFormat,
    path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ImportFormat {
    Jsonl,
    Csv,
}

#[derive(Debug, Deserialize)]
struct ProjectWechatAccount {
    token: String,
    #[serde(rename = "baseUrl")]
    base_url: String,
    #[serde(rename = "accountId")]
    account_id: String,
    #[serde(rename = "userId", default)]
    user_id: Option<String>,
}

fn usage() -> &'static str {
    "Usage:\n  magiclaw                Start daemon mode\n  magiclaw --mcp          Start MCP server mode\n  magiclaw send --message <text> [--to <recipient>] [--data-dir <wechat-dir>]\n  magiclaw auth issue --project <project_id> --name <client_name> --scopes send,window_status --ttl-secs <secs>\n  magiclaw auth list [--project <project_id>]\n  magiclaw auth revoke --token <raw_token>\n  magiclaw bind import (--jsonl <path> | --csv <path>)\n  magiclaw push import (--jsonl <path> | --csv <path>)\n  magiclaw push run --job <job_id>\n  magiclaw project list\n  magiclaw binding list --project <project_key>\n\nEnvironment:\n  MAGICLAW_DB_PATH     SQLite database path shared by daemon and auth commands\n  WECHAT_CHANNEL_DIR    Default WeChat data directory (fallback: ~/.claude/channels/wechat)\n  MAGICLAW_API_TOKEN    Optional bearer token for localhost daemon /api/send and /api/window_status"
}
fn resolve_db_path() -> String {
    env::var("MAGICLAW_DB_PATH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| AppConfig::default().db_path)
}

fn default_wechat_data_dir() -> PathBuf {
    if let Ok(dir) = env::var("WECHAT_CHANNEL_DIR") {
        return PathBuf::from(dir);
    }

    if let Ok(home) = env::var("HOME") {
        return Path::new(&home).join(".claude").join("channels").join("wechat");
    }

    PathBuf::from(".claude/channels/wechat")
}

fn resolve_wechat_data_dir(path: Option<&str>) -> PathBuf {
    match path {
        Some(value) if !value.trim().is_empty() => {
            let candidate = PathBuf::from(value);
            if candidate.is_dir() {
                candidate
            } else {
                candidate.parent().map(Path::to_path_buf).unwrap_or(candidate)
            }
        }
        _ => default_wechat_data_dir(),
    }
}

fn load_project_wechat_account(data_dir: &Path) -> Result<ProjectWechatAccount, Box<dyn std::error::Error>> {
    let account_path = data_dir.join("account.json");
    let content = fs::read_to_string(&account_path)?;
    let account: ProjectWechatAccount = serde_json::from_str(&content)?;
    Ok(account)
}

fn load_project_context_tokens(data_dir: &Path) -> Result<std::collections::HashMap<String, String>, Box<dyn std::error::Error>> {
    let ctx_path = data_dir.join("context_tokens.json");
    if !ctx_path.exists() {
        return Ok(HashMap::new());
    }

    let content = fs::read_to_string(&ctx_path)?;
    let tokens: HashMap<String, String> = serde_json::from_str(&content)?;
    Ok(tokens)
}

fn save_project_context_tokens(
    data_dir: &Path,
    tokens: &std::collections::HashMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let ctx_path = data_dir.join("context_tokens.json");
    let content = serde_json::to_string_pretty(tokens)?;
    fs::write(&ctx_path, format!("{}\n", content))?;
    Ok(())
}

fn parse_cli_args(args: &[String]) -> Result<CliCommand, String> {
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

    if args[1] != "send" {
        return Err(format!("unknown command: {}\n{}", args[1], usage()));
    }

    let mut data_dir = None::<String>;
    let mut to: Option<String> = None;
    let mut message: Option<String> = None;

    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--data-dir" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| format!("missing value for --data-dir\n{}", usage()))?;
                data_dir = Some(value.clone());
            }
            "--to" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| format!("missing value for --to\n{}", usage()))?;
                to = Some(value.clone());
            }
            "--message" | "--text" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| format!("missing value for --message\n{}", usage()))?;
                message = Some(value.clone());
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

    let message = message.ok_or_else(|| format!("missing required flag: --message\n{}", usage()))?;

    Ok(CliCommand::Send(SendCommand {
        data_dir: data_dir.unwrap_or_default(),
        to,
        message,
    }))
}

/// Parse `import (--jsonl <path> | --csv <path>)` shared by bind/push.
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
                let fmt = if rest[index] == "--jsonl" { ImportFormat::Jsonl } else { ImportFormat::Csv };
                index += 1;
                let value = rest.get(index).ok_or_else(|| format!("missing path for {}\n{}", rest[index - 1], usage()))?;
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
                Err(format!("usage: magiclaw push run --job <job_id>\n{}", usage()))
            }
        }
        _ => Err(format!("usage: magiclaw push (import ... | run --job <id>)\n{}", usage())),
    }
}

fn parse_binding_args(args: &[String]) -> Result<CliCommand, String> {
    if args.len() == 3 && args[0] == "list" && args[1] == "--project" {
        return Ok(CliCommand::BindingList(args[2].clone()));
    }
    Err(format!("usage: magiclaw binding list --project <project_key>\n{}", usage()))
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
                        let value = args.get(index).ok_or_else(|| format!("missing value for --project\n{}", usage()))?;
                        project_id = Some(value.clone());
                    }
                    "--name" => {
                        index += 1;
                        let value = args.get(index).ok_or_else(|| format!("missing value for --name\n{}", usage()))?;
                        client_name = Some(value.clone());
                    }
                    "--scopes" => {
                        index += 1;
                        let value = args.get(index).ok_or_else(|| format!("missing value for --scopes\n{}", usage()))?;
                        scopes = Some(parse_scopes(value)?);
                    }
                    "--ttl-secs" => {
                        index += 1;
                        let value = args.get(index).ok_or_else(|| format!("missing value for --ttl-secs\n{}", usage()))?;
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
                project_id: project_id.ok_or_else(|| format!("missing required flag: --project\n{}", usage()))?,
                client_name: client_name.ok_or_else(|| format!("missing required flag: --name\n{}", usage()))?,
                scopes: scopes.ok_or_else(|| format!("missing required flag: --scopes\n{}", usage()))?,
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
                        let value = args.get(index).ok_or_else(|| format!("missing value for --project\n{}", usage()))?;
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
                        let value = args.get(index).ok_or_else(|| format!("missing value for --token\n{}", usage()))?;
                        token = Some(value.clone());
                    }
                    "--help" | "-h" => return Err(usage().to_string()),
                    other => return Err(format!("unknown flag: {}\n{}", other, usage())),
                }
                index += 1;
            }
            Ok(AuthCommand::Revoke(AuthRevokeCommand {
                token: token.ok_or_else(|| format!("missing required flag: --token\n{}", usage()))?,
            }))
        }
        _ => Err(format!("usage: magiclaw auth (issue|list|revoke)\n{}", usage())),
    }
}

/// Open the SQLite pool used by the binding/push CLI, creating the parent dir.
fn open_db_pool() -> Result<magiclaw::infrastructure::db::DbPool, Box<dyn std::error::Error>> {
    let db_path = resolve_db_path();
    if db_path != ":memory:" {
        if let Some(parent) = Path::new(&db_path).parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
    }
    let conn = magiclaw::infrastructure::db::init_db(&db_path)?;
    Ok(magiclaw::infrastructure::db::DbPool::new(conn))
}

fn open_api_client_registry() -> Result<ApiClientRegistry, Box<dyn std::error::Error>> {
    Ok(ApiClientRegistry::new(open_db_pool()?))
}

fn print_import_summary(label: &str, summary: &magiclaw::application::binding::ImportSummary) {
    println!("{}: total={} success={} failed={}", label, summary.total, summary.success, summary.failed);
    for err in &summary.errors {
        eprintln!("  - {}", err);
    }
}

fn run_bind_import(cmd: &ImportCommand) -> Result<(), Box<dyn std::error::Error>> {
    use magiclaw::application::binding::{import_bindings_csv, import_bindings_jsonl};
    let db = open_db_pool()?;
    let summary = match cmd.format {
        ImportFormat::Jsonl => import_bindings_jsonl(&db, &cmd.path)?,
        ImportFormat::Csv => import_bindings_csv(&db, &cmd.path)?,
    };
    print_import_summary("bind import", &summary);
    Ok(())
}

fn run_push_import(cmd: &ImportCommand) -> Result<(), Box<dyn std::error::Error>> {
    use magiclaw::application::push::{import_pushes, parse_pushes_csv, parse_pushes_jsonl};
    let db = open_db_pool()?;
    let (records, format_tag) = match cmd.format {
        ImportFormat::Jsonl => (parse_pushes_jsonl(&cmd.path)?, "jsonl"),
        ImportFormat::Csv => (parse_pushes_csv(&cmd.path)?, "csv"),
    };
    let (job_id, summary) = import_pushes(&db, format_tag, &cmd.path, &records)?;
    print_import_summary("push import", &summary);
    println!("job_id={}", job_id);
    println!("run it with: magiclaw push run --job {}", job_id);
    Ok(())
}

fn run_push_run(job_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    use magiclaw::application::push::run_push_job;
    let db = open_db_pool()?;
    let summary = run_push_job(&db, job_id)?;
    println!(
        "push run: job={} items={} queued={} failed={} enqueued_messages={}",
        summary.job_id, summary.total_items, summary.queued_items, summary.failed_items, summary.enqueued_messages
    );
    Ok(())
}

fn run_project_list() -> Result<(), Box<dyn std::error::Error>> {
    use magiclaw::application::binding::list_projects;
    let db = open_db_pool()?;
    let projects = list_projects(&db)?;
    if projects.is_empty() {
        println!("(no projects)");
        return Ok(());
    }
    for p in projects {
        println!("{}\t{}\tbindings={}", p.project_key, p.project_name, p.binding_count);
    }
    Ok(())
}

fn run_binding_list(project_key: &str) -> Result<(), Box<dyn std::error::Error>> {
    use magiclaw::application::binding::list_bindings;
    let db = open_db_pool()?;
    let bindings = list_bindings(&db, project_key)?;
    if bindings.is_empty() {
        println!("(no active bindings for {})", project_key);
        return Ok(());
    }
    for b in bindings {
        println!(
            "{}\t{}\t{}\t{}\tsource={}",
            b.channel, b.peer_id, b.conversation_id, b.conversation_type, b.bind_source
        );
    }
    Ok(())
}

fn run_auth_issue(cmd: &AuthIssueCommand) -> Result<(), Box<dyn std::error::Error>> {
    let registry = open_api_client_registry()?;
    let issued = registry.issue_token(
        &cmd.project_id,
        &cmd.client_name,
        &cmd.scopes,
        cmd.ttl_secs,
        None,
    )?;
    println!(
        "project_id={} client_name={} expires_at={} scopes={}",
        issued.record.project_id,
        issued.record.client_name,
        issued.record.expires_at,
        issued.record.scopes.join(",")
    );
    println!("token={}", issued.raw_token);
    Ok(())
}

fn run_auth_list(cmd: &AuthListCommand) -> Result<(), Box<dyn std::error::Error>> {
    let registry = open_api_client_registry()?;
    let rows = registry.list_tokens(cmd.project_id.as_deref())?;
    if rows.is_empty() {
        println!("(no api clients)");
        return Ok(());
    }
    for row in rows {
        println!(
            "{}\t{}\t{}\texpires_at={}\trevoked_at={:?}\tscopes={}",
            row.project_id,
            row.client_name,
            row.id,
            row.expires_at,
            row.revoked_at,
            row.scopes.join(",")
        );
    }
    Ok(())
}

fn run_auth_revoke(cmd: &AuthRevokeCommand) -> Result<(), Box<dyn std::error::Error>> {
    let registry = open_api_client_registry()?;
    let revoked = registry.revoke_token(&cmd.token)?;
    println!("revoked={}", revoked);
    Ok(())
}

fn load_runtime_config() -> AppConfig {
    let mut config = AppConfig::default();
    config.db_path = resolve_db_path();
    let data_dir = resolve_wechat_data_dir(None);

    match load_project_wechat_account(&data_dir) {
        Ok(account) => {
            let context_tokens = load_project_context_tokens(&data_dir).unwrap_or_default();
            let context_token = account
                .user_id
                .as_ref()
                .and_then(|u| context_tokens.get(u).cloned())
                .or_else(|| context_tokens.values().next().cloned())
                .unwrap_or_default();

            config.wechat.enabled = true;
            config.wechat.base_url = account.base_url;
            config.wechat.token = account.token;
            config.wechat.account_id = account.account_id;
            config.wechat.context_token = context_token;

            tracing::info!(
                data_dir = %data_dir.display(),
                account_id = %config.wechat.account_id,
                has_context_token = !config.wechat.context_token.is_empty(),
                "loaded wechat runtime config"
            );
        }
        Err(e) => {
            tracing::warn!(
                data_dir = %data_dir.display(),
                error = %e,
                "wechat account config not found; runtime will use skeleton channel"
            );
        }
    }

    // Feishu runtime config from env (optional).
    if let Ok(enabled) = env::var("MAGICLAW_FEISHU_ENABLED") {
        let v = enabled.trim().to_ascii_lowercase();
        config.feishu.enabled = matches!(v.as_str(), "1" | "true" | "yes" | "on");
    }
    if let Ok(v) = env::var("MAGICLAW_FEISHU_ACCOUNT_ID") {
        if !v.trim().is_empty() {
            config.feishu.account_id = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("MAGICLAW_FEISHU_BASE_URL") {
        if !v.trim().is_empty() {
            config.feishu.base_url = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("MAGICLAW_FEISHU_APP_ID") {
        if !v.trim().is_empty() {
            config.feishu.app_id = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("MAGICLAW_FEISHU_APP_SECRET") {
        if !v.trim().is_empty() {
            config.feishu.app_secret = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("MAGICLAW_FEISHU_TENANT_ACCESS_TOKEN") {
        if !v.trim().is_empty() {
            config.feishu.tenant_access_token = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("MAGICLAW_FEISHU_RECEIVE_ID_TYPE") {
        if !v.trim().is_empty() {
            config.feishu.receive_id_type = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("MAGICLAW_FEISHU_VERIFICATION_TOKEN") {
        if !v.trim().is_empty() {
            config.feishu.verification_token = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("MAGICLAW_FEISHU_SIGNING_SECRET") {
        if !v.trim().is_empty() {
            config.feishu.signing_secret = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("MAGICLAW_FEISHU_ACCOUNTS_JSON") {
        if !v.trim().is_empty() {
            match serde_json::from_str::<Vec<magiclaw::infrastructure::config::FeishuConfig>>(v.trim()) {
                Ok(accounts) => {
                    config.feishu_accounts = accounts;
                    tracing::info!(count = config.feishu_accounts.len(), "loaded feishu multi-account config from env");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "invalid MAGICLAW_FEISHU_ACCOUNTS_JSON; ignored");
                }
            }
        }
    }

    // AI backend selection (Phase 4): MAGICLAW_AI_BACKEND overrides the config.
    // Defaults to "echo"; set to "claude_code" to invoke the local claude CLI.
    if let Ok(backend) = env::var("MAGICLAW_AI_BACKEND") {
        if !backend.trim().is_empty() {
            config.ai.backend = backend.trim().to_string();
        }
    }

    config
}

/// Process-global singleton guard for long-running modes (daemon/mcp).
///
/// We lock a file under the DB directory so another process cannot start
/// another resident runtime against the same workspace state.
struct SingletonGuard {
    _lock_file: std::fs::File,
}

fn acquire_singleton(mode: &str, config: &AppConfig) -> Result<SingletonGuard, Box<dyn std::error::Error>> {
    let db_path = PathBuf::from(&config.db_path);
    let lock_dir = db_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    fs::create_dir_all(&lock_dir)?;
    let lock_path = lock_dir.join("magiclaw.instance.lock");
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)?;

    if let Err(e) = file.try_lock_exclusive() {
        return Err(format!(
            "{} mode refused: another magiclaw instance is already running (lock: {}) ({})",
            mode,
            lock_path.display(),
            e
        )
        .into());
    }

    // Best-effort metadata for debugging stale locks / owner PID.
    use std::io::Write;
    file.set_len(0)?;
    writeln!(file, "pid={}", std::process::id())?;
    writeln!(file, "mode={}", mode)?;
    writeln!(file, "cwd={}", std::env::current_dir()?.display())?;

    Ok(SingletonGuard { _lock_file: file })
}

/// Attempt to deliver a message through the locally-running magiclaw daemon's HTTP API.
/// Returns Ok(message) on success, with distinct error kinds for routing decisions.
enum DaemonSendError {
    Unreachable(String),
    Rejected(String),
}

async fn try_daemon_api_send(
    api_addr: &str,
    to: &str,
    text: &str,
    context_token: Option<&str>,
    api_token: Option<&str>,
) -> Result<(String, Option<String>), DaemonSendError> {
    #[derive(Serialize)]
    struct ApiSendRequest<'a> {
        to: &'a str,
        text: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_token: Option<&'a str>,
    }
    #[derive(Deserialize)]
    struct ApiSendResponse {
        ok: bool,
        #[serde(default)]
        context_token: Option<String>,
        #[serde(default)]
        error: Option<String>,
    }

    let url = format!("http://{}/api/send", api_addr);
    let client = reqwest::Client::builder()
        // /api/send may legitimately block while waiting for a fresh inbound token.
        // Keep the request timeout above the daemon wait window so the CLI does
        // not cut off a valid recovery path too early.
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(360))
        .build()
        .map_err(|e| DaemonSendError::Unreachable(e.to_string()))?;

    // Daemon might still be binding right after startup; retry only a few times
    // for connect failures, but let an accepted request run to completion.
    let mut last_err = String::new();
    let mut resp_opt = None;
    for _ in 0..3 {
        let mut builder = client.post(&url).json(&ApiSendRequest {
                to,
                text,
                context_token,
            });
        if let Some(token) = api_token {
            if !token.trim().is_empty() {
                builder = builder.bearer_auth(token.trim());
            }
        }
        match builder.send().await {
            Ok(resp) => {
                resp_opt = Some(resp);
                break;
            }
            Err(e) => {
                last_err = e.to_string();
                if e.is_timeout() {
                    return Err(DaemonSendError::Rejected(
                        "daemon send timed out while waiting for inbound token".to_string(),
                    ));
                }
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            }
        }
    }

    let resp = resp_opt.ok_or_else(|| DaemonSendError::Unreachable(format!("daemon unreachable: {}", last_err)))?;

    let status = resp.status();
    let body_text = resp
        .text()
        .await
        .map_err(|e| DaemonSendError::Rejected(format!("daemon bad response: {}", e)))?;

    let body: ApiSendResponse = serde_json::from_str(&body_text).map_err(|_| {
        let trimmed = body_text.trim();
        let detail = if trimmed.is_empty() {
            format!("daemon returned status {} with empty body", status)
        } else {
            format!("daemon returned status {}: {}", status, trimmed)
        };
        DaemonSendError::Rejected(detail)
    })?;

    if status.is_success() && body.ok {
        Ok((format!("message_id=<daemon>, to={}", to), body.context_token))
    } else {
        Err(DaemonSendError::Rejected(
            body.error
                .unwrap_or_else(|| format!("daemon returned status {}", status)),
        ))
    }
}

async fn run_send_command(cmd: &SendCommand) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = resolve_wechat_data_dir((!cmd.data_dir.is_empty()).then_some(cmd.data_dir.as_str()));
    let account = load_project_wechat_account(&data_dir)?;
    let mut context_tokens = load_project_context_tokens(&data_dir)?;

    let recipient = if let Some(to) = cmd.to.as_ref() {
        if to.trim().is_empty() {
            return Err("--to 不能为空".into());
        }
        to.clone()
    } else {
        context_tokens
            .keys()
            .next()
            .cloned()
            .or(account.user_id.clone())
            .ok_or("无法从 context_tokens.json 或 account.json 推断收件人，请先在微信里给 ClawBot 发一条消息后重试")?
    };

    tracing::info!(bot_id = %account.account_id, recipient = %recipient, "sending wechat message");

    let context_token = context_tokens
        .get(&recipient)
        .cloned()
        .or_else(|| context_tokens.values().next().cloned())
        .unwrap_or_default();

    // Strategy:
    //   1. Try daemon HTTP API (127.0.0.1:18011) — daemon owns the live session
    //      and refreshes peer tokens itself.
    //   2. Fall back to direct ilink call using the stored context_token.
    let api_addr = env::var("MAGICLAW_API_ADDR").unwrap_or_else(|_| DEFAULT_API_ADDR.to_string());
    let api_token = env::var("MAGICLAW_API_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    match try_daemon_api_send(
        &api_addr,
        &recipient,
        &cmd.message,
        (!context_token.is_empty()).then_some(context_token.as_str()),
        api_token.as_deref(),
    )
    .await
    {
        Ok((result, daemon_context_token)) => {
            if let Some(token) = daemon_context_token.as_deref().map(str::trim).filter(|token| !token.is_empty()) {
                context_tokens.insert(recipient.clone(), token.to_string());
                let _ = save_project_context_tokens(&data_dir, &context_tokens);
            }
            println!("send ok (via daemon): {}", result);
            return Ok(());
        }
        Err(DaemonSendError::Rejected(err)) => {
            let lower = err.to_ascii_lowercase();
            if lower.contains("unauthorized") || lower.contains("401") {
                tracing::warn!(error = %err, "daemon rejected send, falling back to direct ilink send");
            } else {
                return Err(format!("daemon send failed: {}", err).into());
            }
        }
        Err(DaemonSendError::Unreachable(err)) => {
            tracing::warn!(error = %err, "daemon unreachable, falling back to direct ilink send");
        }
    }

    // Daemon not running or unreachable — fall back to direct ilink.
    tracing::info!("using direct ilink fallback");

    let client = reqwest::Client::new();
    let send_cfg = ILinkSendConfig {
        base_url: account.base_url.clone(),
        token: account.token.clone(),
        from_user_id: account.account_id.clone(),
        context_token,
        channel_version: "0.1.0".into(),
        timeout_ms: 15_000,
        keepalive_timeout_ms: 4_000,
    };

    let send_result = send_text_via_ilink(&client, &send_cfg, &recipient, &cmd.message)
        .await
        .map_err(|e| format!("wechat send failed: {}", e))?;

    if let Some(token) = send_result
        .get("msg")
        .and_then(|v| v.get("context_token"))
        .and_then(|v| v.as_str())
        .or_else(|| send_result.get("context_token").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        context_tokens.insert(recipient.clone(), token.to_string());
        let _ = save_project_context_tokens(&data_dir, &context_tokens);
    }

    let receipt = magiclaw::channels::channel_trait::SendReceipt {
        message_id: uuid::Uuid::new_v4().to_string(),
        platform_msg_id: send_result
            .get("msg")
            .and_then(|v| v.get("server_id"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
            .or_else(|| send_result.get("server_id").and_then(|v| v.as_str()).map(|v| v.to_string())),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
    };

    println!(
        "send ok: message_id={}, platform_msg_id={}",
        receipt.message_id,
        receipt.platform_msg_id.as_deref().unwrap_or("<none>")
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Auto-load .env if present; real environment variables still take precedence.
    let _ = dotenvy::dotenv();
    tracing_init::init_tracing("info");

    let args: Vec<String> = env::args().collect();
    match parse_cli_args(&args) {
        Ok(CliCommand::Daemon) => {
            let config = load_runtime_config();
            let _singleton = acquire_singleton("daemon", &config)?;
            let runtime = AppRuntime::new(config.clone())?;
            runtime.start_background().await?;

            let api_addr = env::var("MAGICLAW_API_ADDR").unwrap_or_else(|_| DEFAULT_API_ADDR.to_string());
            if let Err(e) = runtime.start_http_api(&api_addr) {
                tracing::warn!(error = %e, "HTTP API disabled");
            }

            tracing::info!(
                active_conversations = runtime.active_conversations(),
                api_addr = %api_addr,
                "magiclaw started"
            );

            tokio::signal::ctrl_c().await?;
        }
        Ok(CliCommand::Mcp) => {
            let config = load_runtime_config();
            let _singleton = acquire_singleton("mcp", &config)?;
            let runtime = AppRuntime::new(config.clone())?;
            runtime.start_background().await?;

            tracing::info!("starting MCP server on stdio");
            let server = McpServer::new("magiclaw", "0.1.0", runtime.outbox_repo.clone());
            server.run().await;
        }
        Ok(CliCommand::Send(cmd)) => {
            run_send_command(&cmd).await?;
        }
        Ok(CliCommand::Auth(cmd)) => match cmd {
            AuthCommand::Issue(cmd) => run_auth_issue(&cmd)?,
            AuthCommand::List(cmd) => run_auth_list(&cmd)?,
            AuthCommand::Revoke(cmd) => run_auth_revoke(&cmd)?,
        },
        Ok(CliCommand::BindImport(cmd)) => {
            run_bind_import(&cmd)?;
        }
        Ok(CliCommand::PushImport(cmd)) => {
            run_push_import(&cmd)?;
        }
        Ok(CliCommand::PushRun(job_id)) => {
            run_push_run(&job_id)?;
        }
        Ok(CliCommand::ProjectList) => {
            run_project_list()?;
        }
        Ok(CliCommand::BindingList(project_key)) => {
            run_binding_list(&project_key)?;
        }
        Err(message) => {
            eprintln!("{}", message);
            std::process::exit(1);
        }
    }

    tracing::info!("magiclaw shutting down");
    Ok(())
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
                data_dir: "/tmp/wechat".into(),
                to: None,
                message: "hello".into(),
            })
        );
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
                assert_eq!(cmd.scopes, vec!["send".to_string(), "window_status".to_string()]);
                assert_eq!(cmd.ttl_secs, 3600);
            }
            other => panic!("unexpected parse result: {:?}", other),
        }
    }

    #[test]
    fn default_wechat_data_dir_prefers_env_override() {
        let original = env::var("WECHAT_CHANNEL_DIR").ok();
        env::set_var("WECHAT_CHANNEL_DIR", "/tmp/wechat-data");
        let dir = default_wechat_data_dir();
        assert_eq!(dir, PathBuf::from("/tmp/wechat-data"));
        match original {
            Some(value) => env::set_var("WECHAT_CHANNEL_DIR", value),
            None => env::remove_var("WECHAT_CHANNEL_DIR"),
        }
    }
}
