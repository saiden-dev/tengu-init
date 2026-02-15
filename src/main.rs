//! Tengu Cloud Init - Hetzner VPS Provisioning
//!
//! Provisions a new Hetzner Cloud server with Tengu PaaS installed.

mod providers;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use std::{env, fs, thread};

use anyhow::{Context, Result};
use clap::Parser;
use comfy_table::{presets::UTF8_FULL_CONDENSED, Cell, Color, Table};
use console::{style, Emoji};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use tera::Tera;

use providers::{hetzner::ServerParams, Hetzner};

static LOOKING_GLASS: Emoji<'_, '_> = Emoji("🔍 ", "");
static ROCKET: Emoji<'_, '_> = Emoji("🚀 ", "");
static SPARKLE: Emoji<'_, '_> = Emoji("✨ ", "");
static CHECK: Emoji<'_, '_> = Emoji("✅ ", "✓ ");
static CROSS: Emoji<'_, '_> = Emoji("❌ ", "✗ ");
static GEAR: Emoji<'_, '_> = Emoji("⚙️  ", "");
static FOLDER: Emoji<'_, '_> = Emoji("📁 ", "");

const TEMPLATE: &str = include_str!("../templates/cloud-init.yml.tera");
const DEFAULT_RELEASE: &str = "v0.1.0-a680bf0";

/// Configuration file structure
/// Path: ~/.config/tengu/init.toml (XDG-style, same as main tengu config)
#[derive(Debug, Default, Serialize, Deserialize)]
struct Config {
    #[serde(default)]
    server: ServerConfig,
    #[serde(default)]
    domains: DomainsConfig,
    #[serde(default)]
    cloudflare: CloudflareConfig,
    #[serde(default)]
    resend: ResendConfig,
    #[serde(default)]
    ssh: SshConfig,
    #[serde(default)]
    notifications: NotificationsConfig,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ServerConfig {
    name: Option<String>,
    #[serde(rename = "type")]
    server_type: Option<String>,
    location: Option<String>,
    image: Option<String>,
    release: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DomainsConfig {
    platform: Option<String>,
    apps: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CloudflareConfig {
    api_key: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ResendConfig {
    api_key: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SshConfig {
    public_key: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct NotificationsConfig {
    email: Option<String>,
}

#[derive(Parser, Debug)]
#[command(name = "tengu-init", version, about = "Provision Tengu PaaS on Hetzner Cloud")]
struct Args {
    /// Server name
    #[arg()]
    name: Option<String>,

    /// Server type (e.g., cax41, cpx42)
    #[arg(short = 't', long)]
    server_type: Option<String>,

    /// Datacenter location (e.g., hel1, fsn1, nbg1)
    #[arg(short, long)]
    location: Option<String>,

    /// Ubuntu image
    #[arg(long)]
    image: Option<String>,

    /// Platform domain (e.g., tengu.to)
    #[arg(long)]
    domain_platform: Option<String>,

    /// Apps domain (e.g., tengu.host)
    #[arg(long)]
    domain_apps: Option<String>,

    /// Cloudflare API key
    #[arg(long)]
    cf_api_key: Option<String>,

    /// Cloudflare email
    #[arg(long)]
    cf_email: Option<String>,

    /// Resend API key
    #[arg(long)]
    resend_api_key: Option<String>,

    /// Notification email
    #[arg(long)]
    notify_email: Option<String>,

    /// SSH public key
    #[arg(long)]
    ssh_key: Option<String>,

    /// Tengu release tag
    #[arg(long)]
    release: Option<String>,

    /// Config file path
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Force recreation if server exists
    #[arg(short, long)]
    force: bool,

    /// Dry run - show config without creating
    #[arg(long)]
    dry_run: bool,

    /// Show config file path and exit
    #[arg(long)]
    show_config: bool,
}

/// Resolved configuration (config file + CLI args + env vars merged)
struct ResolvedConfig {
    name: String,
    server_type: String,
    location: String,
    image: String,
    domain_platform: String,
    domain_apps: String,
    cf_api_key: String,
    cf_email: String,
    resend_api_key: String,
    notify_email: String,
    ssh_key: String,
    release: String,
}

/// Config path - uses same XDG-style path as main tengu config
/// Always ~/.config/tengu/init.toml (even on macOS, for consistency)
fn config_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tengu")
        .join("init.toml")
}

fn load_config(path: Option<&PathBuf>) -> Result<Config> {
    let path = path.cloned().unwrap_or_else(config_path);

    if path.exists() {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config: {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("Failed to parse config: {}", path.display()))
    } else {
        Ok(Config::default())
    }
}

fn resolve_config(args: &Args, config: &Config) -> Result<ResolvedConfig> {
    // Priority: CLI args > env vars > config file > defaults

    let cf_api_key = args.cf_api_key.clone()
        .or_else(|| env::var("CF_API_KEY").ok())
        .or_else(|| config.cloudflare.api_key.clone());

    let cf_email = args.cf_email.clone()
        .or_else(|| env::var("CF_EMAIL").ok())
        .or_else(|| config.cloudflare.email.clone());

    let resend_api_key = args.resend_api_key.clone()
        .or_else(|| env::var("RESEND_API_KEY").ok())
        .or_else(|| config.resend.api_key.clone());

    let ssh_key = args.ssh_key.clone()
        .or_else(|| env::var("SSH_PUBLIC_KEY").ok())
        .or_else(|| config.ssh.public_key.clone());

    // Validate required fields
    let missing: Vec<&str> = [
        cf_api_key.is_none().then_some("cloudflare.api_key"),
        cf_email.is_none().then_some("cloudflare.email"),
        resend_api_key.is_none().then_some("resend.api_key"),
    ]
    .into_iter()
    .flatten()
    .collect();

    if !missing.is_empty() {
        let config_path = config_path();
        eprintln!("{} Missing required credentials: {}", CROSS, missing.join(", "));
        eprintln!();
        eprintln!("Add to config file: {}", style(config_path.display()).cyan());
        eprintln!();
        eprintln!("  [cloudflare]");
        eprintln!("  api_key = \"your-api-key\"");
        eprintln!("  email = \"your-email\"");
        eprintln!();
        eprintln!("  [resend]");
        eprintln!("  api_key = \"re_xxx\"");
        std::process::exit(1);
    }

    Ok(ResolvedConfig {
        name: args.name.clone()
            .or_else(|| config.server.name.clone())
            .unwrap_or_else(|| "tengu".to_string()),
        server_type: args.server_type.clone()
            .or_else(|| config.server.server_type.clone())
            .unwrap_or_else(|| "cax41".to_string()),
        location: args.location.clone()
            .or_else(|| config.server.location.clone())
            .unwrap_or_else(|| "hel1".to_string()),
        image: args.image.clone()
            .or_else(|| config.server.image.clone())
            .unwrap_or_else(|| "ubuntu-24.04".to_string()),
        domain_platform: args.domain_platform.clone()
            .or_else(|| config.domains.platform.clone())
            .unwrap_or_else(|| "tengu.to".to_string()),
        domain_apps: args.domain_apps.clone()
            .or_else(|| config.domains.apps.clone())
            .unwrap_or_else(|| "tengu.host".to_string()),
        cf_api_key: cf_api_key.unwrap(),
        cf_email: cf_email.unwrap(),
        resend_api_key: resend_api_key.unwrap(),
        notify_email: args.notify_email.clone()
            .or_else(|| config.notifications.email.clone())
            .unwrap_or_else(|| "admin@example.com".to_string()),
        ssh_key: ssh_key.unwrap_or_default(),
        release: args.release.clone()
            .or_else(|| config.server.release.clone())
            .unwrap_or_else(|| DEFAULT_RELEASE.to_string()),
    })
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Show config path and exit
    if args.show_config {
        let path = args.config.clone().unwrap_or_else(config_path);
        println!("{} Config: {}", FOLDER, path.display());
        if path.exists() {
            println!("  {} exists", CHECK);
        } else {
            println!("  {} not found (will use defaults)", style("!").yellow());
        }
        return Ok(());
    }

    // Load config file
    let config = load_config(args.config.as_ref())?;

    // Resolve final configuration
    let resolved = resolve_config(&args, &config)?;

    // Print banner
    print_banner();

    // Get server type info
    let type_info = Hetzner::server_type_info(&resolved.server_type)?;

    // Print configuration table
    print_config_table(&resolved, &type_info);

    if args.dry_run {
        println!("\n{} Dry run - not creating server", style("ℹ").cyan());
        print_cloud_init_preview(&resolved)?;
        return Ok(());
    }

    // Check if server exists
    if Hetzner::server_exists(&resolved.name)? {
        println!(
            "\n{} Server '{}' already exists",
            style("!").yellow(),
            resolved.name
        );

        if !args.force {
            let confirm = dialoguer::Confirm::new()
                .with_prompt("Delete and recreate?")
                .default(false)
                .interact()?;

            if !confirm {
                println!("Aborted.");
                return Ok(());
            }
        }

        Hetzner::delete_server(&resolved.name)?;
    }

    // Generate cloud-init
    println!("\n{} Generating cloud-init configuration...", GEAR);
    let cloud_init = render_cloud_init(&resolved)?;

    // Write to temp file
    let temp_file = tempfile::Builder::new()
        .prefix("cloud-init-")
        .suffix(".yml")
        .tempfile()?;
    std::fs::write(temp_file.path(), &cloud_init)?;

    // Create server
    println!("\n{} Creating server...", ROCKET);
    let params = ServerParams {
        name: &resolved.name,
        server_type: &resolved.server_type,
        image: &resolved.image,
        location: &resolved.location,
        cloud_init_path: temp_file.path(),
    };
    let ip = Hetzner::create_server(&params)?;

    println!("  {} IP: {}", style("→").dim(), style(&ip).cyan());

    // Remove old host key
    Hetzner::clear_host_key(&ip);

    // Wait for SSH
    wait_for_ssh(&ip)?;

    // Stream cloud-init progress
    stream_cloud_init_logs(&ip)?;

    // Print success
    print_success(&resolved, &ip);

    Ok(())
}

fn print_banner() {
    println!();
    println!(
        "{}",
        style("╔═══════════════════════════════════════╗")
            .cyan()
            .bold()
    );
    println!(
        "{}",
        style("║       TENGU CLOUD PROVISIONING        ║")
            .cyan()
            .bold()
    );
    println!(
        "{}",
        style("╚═══════════════════════════════════════╝")
            .cyan()
            .bold()
    );
}

fn print_config_table(cfg: &ResolvedConfig, type_info: &str) {
    println!("\n{} Configuration\n", style("▸").blue().bold());

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec![
        Cell::new("Setting").fg(Color::Cyan),
        Cell::new("Value").fg(Color::Cyan),
    ]);

    table.add_row(vec!["Name", &cfg.name]);
    table.add_row(vec!["Type", &format!("{} ({})", cfg.server_type, type_info)]);
    table.add_row(vec!["Location", &cfg.location]);
    table.add_row(vec!["Image", &cfg.image]);
    table.add_row(vec!["Cloudflare", &cfg.cf_email]);
    table.add_row(vec!["Resend", &format!("{}...", &cfg.resend_api_key[..12.min(cfg.resend_api_key.len())])]);
    table.add_row(vec!["Domains", &format!("{}, {}", cfg.domain_platform, cfg.domain_apps)]);
    table.add_row(vec!["Release", &cfg.release]);

    println!("{table}");
}

fn render_cloud_init(cfg: &ResolvedConfig) -> Result<String> {
    let mut tera = Tera::default();
    tera.add_raw_template("cloud-init", TEMPLATE)?;

    let mut context = tera::Context::new();
    context.insert("domain_platform", &cfg.domain_platform);
    context.insert("domain_apps", &cfg.domain_apps);
    context.insert("domain_api", &format!("api.{}", cfg.domain_platform));
    context.insert("domain_docs", &format!("docs.{}", cfg.domain_platform));
    context.insert("domain_git", &format!("git.{}", cfg.domain_platform));
    context.insert("cf_api_key", &cfg.cf_api_key);
    context.insert("cf_email", &cfg.cf_email);
    context.insert("resend_api_key", &cfg.resend_api_key);
    context.insert("ssh_key", &cfg.ssh_key);
    context.insert("notify_email", &cfg.notify_email);
    context.insert("tengu_release", &cfg.release);

    tera.render("cloud-init", &context)
        .context("Failed to render cloud-init template")
}

fn print_cloud_init_preview(cfg: &ResolvedConfig) -> Result<()> {
    let content = render_cloud_init(cfg)?;
    println!("\n{} Cloud-init preview:\n", LOOKING_GLASS);
    // Show first 50 lines
    for line in content.lines().take(50) {
        println!("  {}", style(line).dim());
    }
    println!("  {}", style("... (truncated)").dim());
    Ok(())
}

fn wait_for_ssh(ip: &str) -> Result<()> {
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap(),
    );
    spinner.set_message("Waiting for SSH...");
    spinner.enable_steady_tick(Duration::from_millis(100));

    loop {
        let status = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                "-o", "LogLevel=ERROR",
                "-o", "ConnectTimeout=5",
                "-o", "BatchMode=yes",
                &format!("chi@{}", ip),
                "true",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        if status.map(|s| s.success()).unwrap_or(false) {
            break;
        }
        thread::sleep(Duration::from_secs(3));
    }

    spinner.finish_with_message(format!("{} SSH ready", CHECK));
    Ok(())
}

fn stream_cloud_init_logs(ip: &str) -> Result<()> {
    println!("\n{}", style("─".repeat(50)).dim());
    println!("{} Cloud-init progress:\n", style("▸").cyan());

    let mut child = Command::new("ssh")
        .args([
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "LogLevel=ERROR",
            &format!("chi@{}", ip),
            "while [ ! -f /var/log/cloud-init-output.log ]; do sleep 1; done; \
             tail -f /var/log/cloud-init-output.log 2>/dev/null & PID=$!; \
             cloud-init status --wait >/dev/null 2>&1; \
             sleep 2; kill $PID 2>/dev/null",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to stream logs")?;

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            if let Ok(line) = line {
                // Filter out noise, show key progress
                if line.contains("Setting up")
                    || line.contains("Unpacking")
                    || line.contains("Created symlink")
                    || line.contains("enabled")
                    || line.contains("Processing")
                    || line.contains("tengu")
                    || line.contains("Tengu")
                {
                    println!("  {}", style(&line).dim());
                }
            }
        }
    }

    let _ = child.wait();
    println!("\n{}", style("─".repeat(50)).dim());
    Ok(())
}

fn print_success(cfg: &ResolvedConfig, ip: &str) {
    println!();
    println!(
        "{}",
        style("╔═══════════════════════════════════════╗")
            .green()
            .bold()
    );
    println!(
        "{}",
        style("║            SERVER READY!              ║")
            .green()
            .bold()
    );
    println!(
        "{}",
        style("╚═══════════════════════════════════════╝")
            .green()
            .bold()
    );
    println!();

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);

    table.add_row(vec![
        Cell::new("SSH").fg(Color::Cyan),
        Cell::new(format!("ssh chi@{}", ip)),
    ]);
    table.add_row(vec![
        Cell::new("API").fg(Color::Cyan),
        Cell::new(format!("https://api.{}", cfg.domain_platform)),
    ]);
    table.add_row(vec![
        Cell::new("Docs").fg(Color::Cyan),
        Cell::new(format!("https://docs.{}", cfg.domain_platform)),
    ]);
    table.add_row(vec![
        Cell::new("Apps").fg(Color::Cyan),
        Cell::new(format!("https://<app>.{}", cfg.domain_apps)),
    ]);

    println!("{table}");
    println!();

    println!("{} Deployment complete!", SPARKLE);
}
