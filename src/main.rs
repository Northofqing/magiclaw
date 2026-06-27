use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use magiclaw::adapters::api_client_registry::ApiClientRegistry;
use magiclaw::adapters::mcp::server::McpServer;
use magiclaw::adapters::sqlite_context_tokens::SqliteContextTokenStore;
use magiclaw::channels::wechat::ilink::{send_text_via_ilink, ILinkSendConfig};
use magiclaw::cli::commands::*;
use magiclaw::cli::parser::parse_cli_args;
use magiclaw::daemon::singleton;
use magiclaw::domain::ports::context_token_store::ContextTokenStore;
use magiclaw::infrastructure::config::AppConfig;
use magiclaw::infrastructure::runtime::AppRuntime;
use magiclaw::infrastructure::tracing_init;
use serde::{Deserialize, Serialize};

/// Default HTTP API address for the magiclaw daemon (matches weclaw convention).
const DEFAULT_API_ADDR: &str = "127.0.0.1:18011";
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
        let legacy_dir = Path::new(&home).join(".claude").join("channels").join("wechat");
        if legacy_dir.exists() {
            return legacy_dir;
        }
    }

    let db_path = resolve_db_path();
    Path::new(&db_path)
        .parent()
        .map(|parent| parent.join("wechat"))
        .unwrap_or_else(|| PathBuf::from(".claude/channels/wechat"))
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

fn open_context_token_store() -> Result<SqliteContextTokenStore, Box<dyn std::error::Error>> {
    Ok(SqliteContextTokenStore::open(resolve_db_path())?)
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

fn open_qr_popup(qrcode_path: &Path) -> Option<std::process::Child> {
    if !cfg!(target_os = "macos") {
        return None;
    }

    let path = qrcode_path.to_str()?;
    std::process::Command::new("qlmanage")
        .args(["-p", path])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
}

fn close_qr_popup(child: &mut Option<std::process::Child>) {
    if let Some(process) = child.as_mut() {
        let _ = process.kill();
        let _ = process.wait();
    }
    *child = None;
}

async fn run_wechat_login(cmd: &WechatLoginCommand) -> Result<(), Box<dyn std::error::Error>> {
    use magiclaw::channels::wechat::ilink::{get_bot_qrcode, get_qrcode_status};
    use qrcode::QrCode;
    
    let data_dir = if let Some(dir) = &cmd.data_dir {
        PathBuf::from(dir)
    } else {
        default_wechat_data_dir()
    };
    
    fs::create_dir_all(&data_dir)?;
    
    // WeChat iLink base URL
    let base_url = "https://ilinkai.weixin.qq.com";
    
    let client = reqwest::Client::new();
    
    // Step 1: Get QR code
    println!("正在获取二维码...");
    let qrcode_resp = get_bot_qrcode(&client, base_url).await?;

    let scan_value = qrcode_resp
        .qrcode_img_content
        .clone()
        .unwrap_or_else(|| qrcode_resp.qrcode.clone());
    
    // Generate QR code image and save
    let qr_code = QrCode::new(scan_value.clone())?;
    let image = qr_code.render::<image::Rgb<u8>>()
        .min_dimensions(200, 200)
        .build();
    
    let qrcode_path = data_dir.join("qrcode.png");
    image.save(&qrcode_path)?;
    println!("✓ 二维码已生成: {}", qrcode_path.display());

    let mut qr_popup = open_qr_popup(&qrcode_path);
    if qr_popup.is_some() {
        println!("✓ 已弹出二维码窗口，请在微信中扫描");
    }
    
    println!("\n========== 请用微信扫描以下二维码登录 ==========");
    println!("二维码 ID: {}", qrcode_resp.qrcode);
    println!("扫码链接: {}", scan_value);
    println!("图片位置: {}", qrcode_path.display());
    println!("============================================\n");
    
    println!("等待二维码扫描...");
    
    // Step 2: Poll for QR code status
    let max_wait_secs = 300; // 5 minutes timeout
    let poll_interval_secs = 2;
    let mut elapsed_secs = 0;
    
    let (token, base, api_account_id, api_user_id) = loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(poll_interval_secs)).await;
        elapsed_secs += poll_interval_secs;
        
        match get_qrcode_status(&client, base_url, &qrcode_resp.qrcode).await {
            Ok(status_resp) => {
                let status = status_resp.status.unwrap_or_else(|| "wait".to_string());
                match status.as_str() {
                    "wait" | "waiting" => {
                        print!(".");
                        std::io::Write::flush(&mut std::io::stdout())?;
                    }
                    "scaned" | "scanned" => {
                        println!("\n👀 已扫码，请在微信中确认...");
                    }
                    "expired" => {
                        close_qr_popup(&mut qr_popup);
                        return Err("二维码已过期，请重新运行登录命令".into());
                    }
                    "confirmed" => {
                        let token = status_resp
                            .bot_token
                            .clone()
                            .ok_or("登录确认但未返回 bot_token")?;
                        let base = status_resp
                            .baseurl
                            .clone()
                            .unwrap_or_else(|| base_url.to_string());
                        let account_id = status_resp.ilink_bot_id.clone();
                        let user_id = status_resp.ilink_user_id.clone();
                        println!("\n✓ 二维码扫描成功！");
                        close_qr_popup(&mut qr_popup);
                        break (token, base, account_id, user_id);
                    }
                    other => {
                        println!("\n状态: {}", other);
                    }
                }
            }
            Err(e) => {
                // Status check might fail temporarily; just continue polling
                tracing::debug!(error = %e, "qrcode status check error");
            }
        }
        
        if elapsed_secs >= max_wait_secs {
            close_qr_popup(&mut qr_popup);
            return Err(format!("二维码等待超时 ({}秒)", max_wait_secs).into());
        }
    };
    
    println!("\n");
    
    // Step 3: Extract and save to account.json
    let account_id = if let Some(id) = &cmd.account_id {
        id.clone()
    } else if let Some(id) = api_account_id {
        id
    } else {
        "unknown_account_id".to_string()
    };
    
    let account = ProjectWechatAccount {
        token: token.clone(),
        base_url: base.clone(),
        account_id: account_id.clone(),
        user_id: api_user_id,
        saved_at: Some(chrono::Utc::now().to_rfc3339()),
    };
    
    let account_json_path = data_dir.join("account.json");
    let content = serde_json::to_string_pretty(&account)?;
    fs::write(&account_json_path, format!("{}\n", content))?;
    
    println!("✓ 登录成功！");
    println!("账户信息已保存到: {}", account_json_path.display());
    println!("  token: {}...", &token[..std::cmp::min(20, token.len())]);
    println!("  baseUrl: {}", base);
    println!("  accountId: {}", account_id);
    
    println!("\n现在可以运行以下命令启动 daemon:");
    println!("  scripts/daemon-up.sh");
    
    Ok(())
}

fn validate_feishu_config(cfg: &magiclaw::infrastructure::config::FeishuConfig) -> Result<(), String> {
    if !cfg.enabled {
        return Ok(());
    }
    
    // Validate receive_id_type is a known value
    let valid_receive_id_types = ["open_id", "user_id", "chat_id", "union_id", "email"];
    if !valid_receive_id_types.contains(&cfg.receive_id_type.as_str()) {
        return Err(format!(
            "invalid feishu receive_id_type: '{}' (must be one of: {})",
            cfg.receive_id_type,
            valid_receive_id_types.join(", ")
        ));
    }
    
    // Verify token exchange can be attempted (either pre-issued or via app credentials)
    let has_preissued_token = !cfg.tenant_access_token.trim().is_empty();
    let has_app_credentials = !cfg.app_id.trim().is_empty() || !cfg.app_secret.trim().is_empty();
    
    if !has_preissued_token && !has_app_credentials {
        return Err(
            "feishu enabled but no authentication: set either FEISHU_TENANT_ACCESS_TOKEN or both APP_ID + APP_SECRET"
                .into(),
        );
    }
    
    // If using app credentials, both must be present
    if has_app_credentials && (cfg.app_id.trim().is_empty() || cfg.app_secret.trim().is_empty()) {
        return Err(
            "feishu: if using app-based token exchange, both FEISHU_APP_ID and FEISHU_APP_SECRET must be set"
                .into(),
        );
    }
    
    Ok(())
}

#[allow(clippy::field_reassign_with_default)]
fn load_runtime_config() -> AppConfig {
    let mut config = AppConfig::default();
    config.db_path = resolve_db_path();
    let data_dir = resolve_wechat_data_dir(None);
    let token_store = open_context_token_store().ok();

    match load_project_wechat_account(&data_dir) {
        Ok(account) => {
            let context_tokens = token_store
                .as_ref()
                .and_then(|store| store.get_all(&account.account_id).ok())
                .unwrap_or_default();
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
    if let Ok(enabled) = env::var("FEISHU_ENABLED") {
        let v = enabled.trim().to_ascii_lowercase();
        config.feishu.enabled = matches!(v.as_str(), "1" | "true" | "yes" | "on");
    }
    if let Ok(v) = env::var("FEISHU_ACCOUNT_ID") {
        if !v.trim().is_empty() {
            config.feishu.account_id = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("FEISHU_BASE_URL") {
        if !v.trim().is_empty() {
            config.feishu.base_url = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("FEISHU_APP_ID") {
        if !v.trim().is_empty() {
            config.feishu.app_id = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("FEISHU_APP_SECRET") {
        if !v.trim().is_empty() {
            config.feishu.app_secret = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("FEISHU_TENANT_ACCESS_TOKEN") {
        if !v.trim().is_empty() {
            config.feishu.tenant_access_token = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("FEISHU_RECEIVE_ID_TYPE") {
        if !v.trim().is_empty() {
            config.feishu.receive_id_type = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("FEISHU_VERIFICATION_TOKEN") {
        if !v.trim().is_empty() {
            config.feishu.verification_token = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("FEISHU_SIGNING_SECRET") {
        if !v.trim().is_empty() {
            config.feishu.signing_secret = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("FEISHU_ACCOUNTS_JSON") {
        if !v.trim().is_empty() {
            match serde_json::from_str::<Vec<magiclaw::infrastructure::config::FeishuConfig>>(v.trim()) {
                Ok(accounts) => {
                    config.feishu_accounts = accounts;
                    tracing::info!(count = config.feishu_accounts.len(), "loaded feishu multi-account config from env");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "invalid FEISHU_ACCOUNTS_JSON; ignored");
                }
            }
        }
    }
    
    // Validate Feishu configuration at startup (MUST)
    if let Err(e) = validate_feishu_config(&config.feishu) {
        tracing::error!(error = %e, "feishu config validation failed");
        panic!("feishu config validation failed: {}", e);
    }
    for (idx, cfg) in config.feishu_accounts.iter().enumerate() {
        if let Err(e) = validate_feishu_config(cfg) {
            tracing::error!(account_index = idx, error = %e, "feishu multi-account config validation failed");
            panic!("feishu multi-account[{}] config validation failed: {}", idx, e);
        }
    }
    
    // Log Feishu configuration status
    if config.feishu.enabled {
        tracing::info!(
            account_id = %config.feishu.account_id,
            base_url = %config.feishu.base_url,
            receive_id_type = %config.feishu.receive_id_type,
            has_app_id = !config.feishu.app_id.is_empty(),
            has_preissued_token = !config.feishu.tenant_access_token.is_empty(),
            has_verification_token = !config.feishu.verification_token.is_empty(),
            has_signing_secret = !config.feishu.signing_secret.is_empty(),
            "feishu configuration loaded and validated"
        );
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
    match cmd.channel {
        SendChannel::Wechat => run_send_wechat(cmd).await,
        SendChannel::Feishu => run_send_feishu(cmd).await,
    }
}

/// Build a Feishu config from environment variables for the CLI send path.
fn load_feishu_config_from_env() -> magiclaw::infrastructure::config::FeishuConfig {
    let mut cfg = magiclaw::infrastructure::config::FeishuConfig {
        enabled: true,
        ..Default::default()
    };
    if let Ok(v) = env::var("FEISHU_ACCOUNT_ID") {
        if !v.trim().is_empty() {
            cfg.account_id = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("FEISHU_BASE_URL") {
        if !v.trim().is_empty() {
            cfg.base_url = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("FEISHU_APP_ID") {
        cfg.app_id = v.trim().to_string();
    }
    if let Ok(v) = env::var("FEISHU_APP_SECRET") {
        cfg.app_secret = v.trim().to_string();
    }
    if let Ok(v) = env::var("FEISHU_TENANT_ACCESS_TOKEN") {
        cfg.tenant_access_token = v.trim().to_string();
    }
    if let Ok(v) = env::var("FEISHU_RECEIVE_ID_TYPE") {
        if !v.trim().is_empty() {
            cfg.receive_id_type = v.trim().to_string();
        }
    }
    cfg
}

/// Auto-detect Feishu receive_id_type from a recipient ID prefix.
/// Falls back to None when the prefix is unrecognized.
fn detect_feishu_receive_id_type(recipient: &str) -> Option<&'static str> {
    let r = recipient.trim();
    if r.starts_with("oc_") {
        Some("chat_id")
    } else if r.starts_with("ou_") {
        Some("open_id")
    } else if r.starts_with("on_") {
        Some("union_id")
    } else if r.contains('@') {
        Some("email")
    } else {
        None
    }
}

async fn run_send_feishu(cmd: &SendCommand) -> Result<(), Box<dyn std::error::Error>> {
    use magiclaw::channels::channel_trait::Channel;
    use magiclaw::channels::feishu::channel::FeishuChannel;
    use magiclaw::domain::entities::message::MessageContent;

    let recipient = cmd
        .to
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or("飞书发送必须指定 --to（open_id / chat_id / user_id / union_id / email）")?;

    let mut cfg = load_feishu_config_from_env();

    // Resolve receive_id_type with priority: explicit flag > auto-detect > env/default.
    if let Some(rt) = cmd.receive_id_type.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        cfg.receive_id_type = rt.to_string();
    } else if let Some(detected) = detect_feishu_receive_id_type(recipient) {
        cfg.receive_id_type = detected.to_string();
    }

    if let Err(e) = validate_feishu_config(&cfg) {
        return Err(format!("飞书配置无效: {}", e).into());
    }

    tracing::info!(
        recipient = %recipient,
        receive_id_type = %cfg.receive_id_type,
        account_id = %cfg.account_id,
        "sending feishu message"
    );

    let channel = FeishuChannel::from_config(cfg);
    let content = MessageContent::Text(cmd.message.clone());

    let receipt = channel
        .send_message(recipient, &content)
        .await
        .map_err(|e| format!("feishu send failed: {}", e))?;

    println!(
        "send ok (feishu): message_id={}, platform_msg_id={}",
        receipt.message_id,
        receipt.platform_msg_id.as_deref().unwrap_or("<none>")
    );
    Ok(())
}

async fn run_send_wechat(cmd: &SendCommand) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = resolve_wechat_data_dir((!cmd.data_dir.is_empty()).then_some(cmd.data_dir.as_str()));
    let account = load_project_wechat_account(&data_dir)?;
    let token_store = open_context_token_store()?;
    let mut context_tokens = token_store.get_all(&account.account_id)?;

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
            .ok_or("无法从 context token store 或 account.json 推断收件人，请先在微信里给 ClawBot 发一条消息后重试")?
    };

    tracing::info!(bot_id = %account.account_id, recipient = %recipient, "sending wechat message");

    let context_token = cmd
        .context_token
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
        .or_else(|| {
            context_tokens
                .get(&recipient)
                .cloned()
                .or_else(|| context_tokens.values().next().cloned())
        })
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
                let _ = token_store.set(&account.account_id, &recipient, token);
            } else if let Some(explicit_token) = cmd.context_token.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                context_tokens.insert(recipient.clone(), explicit_token.to_string());
                let _ = token_store.set(&account.account_id, &recipient, explicit_token);
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
        let _ = token_store.set(&account.account_id, &recipient, token);
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
            let _singleton = singleton::acquire_singleton("daemon", &config)?;
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
            let _singleton = singleton::acquire_singleton("mcp", &config)?;
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
        Ok(CliCommand::WeChat(cmd)) => match cmd {
            WechatCommand::Login(cmd) => run_wechat_login(&cmd).await?,
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
    fn detect_feishu_receive_id_type_by_prefix() {
        assert_eq!(detect_feishu_receive_id_type("oc_abc"), Some("chat_id"));
        assert_eq!(detect_feishu_receive_id_type("ou_abc"), Some("open_id"));
        assert_eq!(detect_feishu_receive_id_type("on_abc"), Some("union_id"));
        assert_eq!(detect_feishu_receive_id_type("user@example.com"), Some("email"));
        assert_eq!(detect_feishu_receive_id_type("unknown123"), None);
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
