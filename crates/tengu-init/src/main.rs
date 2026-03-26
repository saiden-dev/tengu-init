//! Tengu Init - Server Provisioning
//!
//! Provisions a server with Tengu `PaaS` installed.
//! - Default: connects to user@host via SSH and provisions
//! - `--hetzner`: creates a Hetzner VPS first, then provisions it

mod providers;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use std::{env, fs, thread};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use comfy_table::{Cell, Color, Table, presets::UTF8_FULL_CONDENSED};
use console::{Emoji, style};
use dialoguer::{Input, Password};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use tengu_provision::{BashRenderer, CloudInitRenderer, Manifest, Renderer, TenguConfig};
use tera::Tera;

use providers::{Baremetal, Hetzner, TunnelConfig, hetzner::ServerParams};

static LOOKING_GLASS: Emoji<'_, '_> = Emoji("🔍 ", "");
static ROCKET: Emoji<'_, '_> = Emoji("🚀 ", "");
static SPARKLE: Emoji<'_, '_> = Emoji("✨ ", "");
static CHECK: Emoji<'_, '_> = Emoji("✅ ", "✓ ");
static GEAR: Emoji<'_, '_> = Emoji("⚙️  ", "");
static FOLDER: Emoji<'_, '_> = Emoji("📁 ", "");

const TEMPLATE: &str = include_str!("../templates/cloud-init.yml.tera");
const DEFAULT_RELEASE: &str = "v0.1.0-22879bf";

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
#[command(
    name = "tengu-init",
    version,
    about = "Provision Tengu PaaS on cloud or baremetal servers"
)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,

    /// SSH destination (user@host), required unless --hetzner
    #[arg()]
    host: Option<String>,

    /// Create Hetzner VPS first (uses hcloud CLI)
    #[arg(long)]
    hetzner: bool,

    /// SSH port
    #[arg(short, long, default_value = "22")]
    port: u16,

    /// Generate script only, don't execute
    #[arg(long)]
    script_only: bool,

    /// Remove Tengu and all installed dependencies from the server
    #[arg(long)]
    remove: bool,

    /// Cloudflare API key
    #[arg(long)]
    cf_api_key: Option<String>,

    /// Cloudflare email
    #[arg(long)]
    cf_email: Option<String>,

    /// Resend API key
    #[arg(long)]
    resend_api_key: Option<String>,

    /// Platform domain
    #[arg(long, default_value = None)]
    domain_platform: Option<String>,

    /// Apps domain
    #[arg(long, default_value = None)]
    domain_apps: Option<String>,

    /// SSH public key
    #[arg(long)]
    ssh_key: Option<String>,

    /// Notification email
    #[arg(long)]
    notify_email: Option<String>,

    /// Tengu release tag
    #[arg(long)]
    release: Option<String>,

    /// Config file path
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Show config file path and exit
    #[arg(long)]
    show_config: bool,

    /// Show config without provisioning
    #[arg(long)]
    dry_run: bool,

    /// Force recreation (Hetzner only)
    #[arg(short, long)]
    force: bool,

    // -- Hetzner-specific options (only relevant with --hetzner) --
    /// Server name (Hetzner only)
    #[arg(short, long)]
    name: Option<String>,

    /// Server type (Hetzner only)
    #[arg(short = 't', long)]
    server_type: Option<String>,

    /// Datacenter location (Hetzner only)
    #[arg(short, long)]
    location: Option<String>,

    /// Ubuntu image (Hetzner only)
    #[arg(long)]
    image: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Show generated provisioning config
    Show(ShowArgs),
}

#[derive(Parser, Debug)]
struct ShowArgs {
    /// Output format
    #[arg(value_enum)]
    format: OutputFormat,
}

/// Output format for show command
#[derive(ValueEnum, Clone, Debug)]
enum OutputFormat {
    /// Cloud-init YAML format
    CloudInit,
    /// Executable bash script
    Bash,
}

/// Resolved provisioning configuration (all credentials present)
struct ResolvedConfig {
    domain_platform: String,
    domain_apps: String,
    cf_api_key: String,
    cf_email: String,
    resend_api_key: String,
    notify_email: String,
    ssh_key: String,
    release: String,
}

/// Hetzner-specific parameters (separate from provisioning config)
struct HetznerParams {
    name: String,
    server_type: String,
    location: String,
    image: String,
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

/// Detect default SSH public key from common locations
fn detect_ssh_key() -> Option<String> {
    let home = env::var("HOME").ok()?;
    let candidates = [
        format!("{home}/.ssh/id_ed25519.pub"),
        format!("{home}/.ssh/id_rsa.pub"),
    ];
    for path in &candidates {
        if let Ok(content) = fs::read_to_string(path) {
            let key = content.trim().to_string();
            if !key.is_empty() {
                return Some(key);
            }
        }
    }
    None
}

/// Check if cloudflared cert.pem exists
fn cloudflared_cert_exists() -> bool {
    let home = env::var("HOME").unwrap_or_default();
    PathBuf::from(home)
        .join(".cloudflared")
        .join("cert.pem")
        .exists()
}

/// Run `cloudflared tunnel login` interactively
fn run_cloudflared_login() -> Result<()> {
    println!(
        "\n{} Cloudflare tunnel authentication required.",
        style("*").cyan()
    );
    println!("  A browser window will open for authentication...\n");

    let status = Command::new("cloudflared")
        .args(["tunnel", "login"])
        .status()
        .context("Failed to run cloudflared - is it installed?")?;

    if !status.success() {
        bail!("cloudflared tunnel login failed");
    }

    println!("  {} Cloudflare tunnel authenticated\n", style("v").green());
    Ok(())
}

/// Resolve configuration interactively
///
/// Priority: CLI args > env vars > config file > interactive prompt > defaults
#[allow(clippy::too_many_lines)]
fn resolve_config(args: &Args, config: &Config) -> Result<ResolvedConfig> {
    // Print header for interactive section
    let needs_interactive = args.cf_email.is_none()
        && env::var("CF_EMAIL").is_err()
        && config.cloudflare.email.is_none();

    if needs_interactive {
        println!(
            "\n{}",
            style("--- Tengu Init \u{2014} Credential Setup ---")
                .cyan()
                .bold()
        );
        println!();
    }

    // 1. Cloudflare email
    let cf_email = args
        .cf_email
        .clone()
        .or_else(|| env::var("CF_EMAIL").ok())
        .or_else(|| config.cloudflare.email.clone())
        .map_or_else(
            || {
                Input::<String>::new()
                    .with_prompt("Cloudflare email")
                    .validate_with(|input: &String| {
                        if input.contains('@') && input.contains('.') {
                            Ok(())
                        } else {
                            Err("Please enter a valid email address")
                        }
                    })
                    .interact_text()
                    .context("Failed to read Cloudflare email")
            },
            Ok,
        )?;

    // 2. Cloudflare API key
    let cf_api_key = args
        .cf_api_key
        .clone()
        .or_else(|| env::var("CF_API_KEY").ok())
        .or_else(|| config.cloudflare.api_key.clone())
        .map_or_else(
            || {
                Password::new()
                    .with_prompt("Cloudflare API key")
                    .interact()
                    .context("Failed to read Cloudflare API key")
            },
            Ok,
        )?;

    // 3. Cloudflare Tunnel auth - check for cert.pem
    if !cloudflared_cert_exists() {
        run_cloudflared_login()?;
    }

    // 4. Resend API key
    let resend_api_key = args
        .resend_api_key
        .clone()
        .or_else(|| env::var("RESEND_API_KEY").ok())
        .or_else(|| config.resend.api_key.clone())
        .map_or_else(
            || {
                Password::new()
                    .with_prompt("Resend API key")
                    .interact()
                    .context("Failed to read Resend API key")
            },
            Ok,
        )?;

    // 5. Platform domain
    let domain_platform = args
        .domain_platform
        .clone()
        .or_else(|| config.domains.platform.clone())
        .map_or_else(
            || {
                Input::<String>::new()
                    .with_prompt("Platform domain")
                    .default("tengu.to".into())
                    .interact_text()
                    .context("Failed to read platform domain")
            },
            Ok,
        )?;

    // 6. Apps domain
    let domain_apps = args
        .domain_apps
        .clone()
        .or_else(|| config.domains.apps.clone())
        .map_or_else(
            || {
                Input::<String>::new()
                    .with_prompt("Apps domain")
                    .default("tengu.host".into())
                    .interact_text()
                    .context("Failed to read apps domain")
            },
            Ok,
        )?;

    // 7. SSH public key
    let detected_key = detect_ssh_key();
    let ssh_key = args
        .ssh_key
        .clone()
        .or_else(|| env::var("SSH_PUBLIC_KEY").ok())
        .or_else(|| config.ssh.public_key.clone())
        .map_or_else(
            || {
                let prompt = Input::<String>::new().with_prompt("SSH public key");
                let prompt = if let Some(ref key) = detected_key {
                    prompt.default(key.clone())
                } else {
                    prompt
                };
                prompt
                    .interact_text()
                    .context("Failed to read SSH public key")
            },
            Ok,
        )?;

    // 8. Notification email (default: CF email)
    let notify_email = args
        .notify_email
        .clone()
        .or_else(|| config.notifications.email.clone())
        .map_or_else(
            || {
                Input::<String>::new()
                    .with_prompt("Notification email")
                    .default(cf_email.clone())
                    .interact_text()
                    .context("Failed to read notification email")
            },
            Ok,
        )?;

    // 9. Tengu release
    let release = args
        .release
        .clone()
        .or_else(|| config.server.release.clone())
        .map_or_else(
            || {
                Input::<String>::new()
                    .with_prompt("Tengu release")
                    .default(DEFAULT_RELEASE.into())
                    .interact_text()
                    .context("Failed to read release tag")
            },
            Ok,
        )?;

    Ok(ResolvedConfig {
        domain_platform,
        domain_apps,
        cf_api_key,
        cf_email,
        resend_api_key,
        notify_email,
        ssh_key,
        release,
    })
}

/// Resolve Hetzner-specific parameters
fn resolve_hetzner_params(args: &Args, config: &Config) -> HetznerParams {
    HetznerParams {
        name: args
            .name
            .clone()
            .or_else(|| config.server.name.clone())
            .unwrap_or_else(|| "tengu".to_string()),
        server_type: args
            .server_type
            .clone()
            .or_else(|| config.server.server_type.clone())
            .unwrap_or_else(|| "cax41".to_string()),
        location: args
            .location
            .clone()
            .or_else(|| config.server.location.clone())
            .unwrap_or_else(|| "hel1".to_string()),
        image: args
            .image
            .clone()
            .or_else(|| config.server.image.clone())
            .unwrap_or_else(|| "ubuntu-24.04".to_string()),
    }
}

#[allow(clippy::too_many_lines)]
fn main() -> Result<()> {
    let args = Args::parse();

    // Show config path and exit
    if args.show_config {
        let path = args.config.clone().unwrap_or_else(config_path);
        println!("{} Config: {}", FOLDER, path.display());
        if path.exists() {
            println!("  {CHECK} exists");
        } else {
            println!("  {} not found (will use defaults)", style("!").yellow());
        }
        return Ok(());
    }

    // Route show subcommand
    if let Some(Commands::Show(show_args)) = &args.command {
        let file_config = load_config(args.config.as_ref())?;
        return run_show(show_args, &file_config);
    }

    // Validate: need either host or --hetzner
    if args.host.is_none() && !args.hetzner {
        bail!(
            "Missing SSH destination. Usage:\n  \
             tengu-init user@host          Provision existing server\n  \
             tengu-init --hetzner          Create Hetzner VPS and provision"
        );
    }

    // Handle --remove: uninstall everything from the target server
    if args.remove {
        let host = args.host.as_ref().ok_or_else(|| {
            anyhow::anyhow!("--remove requires a host argument: tengu-init user@host --remove")
        })?;

        println!();
        println!(
            "{}",
            style("╔═══════════════════════════════════════╗")
                .red()
                .bold()
        );
        println!(
            "{}",
            style("║          TENGU REMOVAL                ║")
                .red()
                .bold()
        );
        println!(
            "{}",
            style("╚═══════════════════════════════════════╝")
                .red()
                .bold()
        );
        println!(
            "\nThis will remove Tengu and all installed dependencies from {}",
            style(host).cyan()
        );
        println!("Including: tengu, caddy, ollama, postgresql, docker, fail2ban\n");

        if !args.force {
            let confirm = dialoguer::Confirm::new()
                .with_prompt("Are you sure?")
                .default(false)
                .interact()?;

            if !confirm {
                println!("Aborted.");
                return Ok(());
            }
        }

        if args.script_only {
            println!("{}", Baremetal::generate_removal_script());
            return Ok(());
        }

        let provider = Baremetal::new(host, args.port);
        provider.remove()?;

        return Ok(());
    }

    // Load config file
    let file_config = load_config(args.config.as_ref())?;

    // Resolve config (CLI > env > config > interactive > defaults)
    let resolved = resolve_config(&args, &file_config)?;

    // Determine the SSH host
    let host = if args.hetzner {
        // Hetzner flow: create server first, get IP
        let hetzner_params = resolve_hetzner_params(&args, &file_config);

        print_banner();
        print_hetzner_config_table(&resolved, &hetzner_params)?;

        if args.dry_run {
            println!("\n{} Dry run - not creating server", style("i").cyan());
            print_cloud_init_preview(&resolved)?;
            return Ok(());
        }

        // Check if server exists
        if Hetzner::server_exists(&hetzner_params.name)? {
            println!(
                "\n{} Server '{}' already exists",
                style("!").yellow(),
                hetzner_params.name
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

            Hetzner::delete_server(&hetzner_params.name)?;
        }

        // Generate cloud-init
        println!("\n{GEAR} Generating cloud-init configuration...");
        let cloud_init = render_cloud_init(&resolved)?;

        // Write to temp file
        let temp_file = tempfile::Builder::new()
            .prefix("cloud-init-")
            .suffix(".yml")
            .tempfile()?;
        std::fs::write(temp_file.path(), &cloud_init)?;

        // Create server
        println!("\n{ROCKET} Creating server...");
        let params = ServerParams {
            name: &hetzner_params.name,
            server_type: &hetzner_params.server_type,
            image: &hetzner_params.image,
            location: &hetzner_params.location,
            cloud_init_path: temp_file.path(),
        };
        let ip = Hetzner::create_server(&params)?;

        println!("  {} IP: {}", style("->").dim(), style(&ip).cyan());

        // Remove old host key
        Hetzner::clear_host_key(&ip);

        // Wait for SSH
        wait_for_ssh(&ip);

        // Stream cloud-init progress
        stream_cloud_init_logs(&ip)?;

        // Print success
        print_success(&resolved, &ip);

        return Ok(());
    } else {
        // Direct SSH host provided
        args.host.clone().unwrap()
    };

    // Extract user from host
    let user = if let Some((u, _)) = host.split_once('@') {
        u.to_string()
    } else {
        "chi".to_string()
    };

    // Build TenguConfig for baremetal provisioning
    let tengu_config = TenguConfig::builder()
        .user(user)
        .domain_platform(&resolved.domain_platform)
        .domain_apps(&resolved.domain_apps)
        .cf_api_key(&resolved.cf_api_key)
        .cf_email(&resolved.cf_email)
        .resend_api_key(&resolved.resend_api_key)
        .notify_email(&resolved.notify_email)
        .ssh_keys(
            if resolved.ssh_key.is_empty() {
                vec![]
            } else {
                vec![resolved.ssh_key.clone()]
            },
        )
        .release(&resolved.release)
        .build();

    // Script-only mode
    if args.script_only {
        let script = Baremetal::generate_script(&tengu_config)?;
        println!("{script}");
        return Ok(());
    }

    // Print banner
    print_banner();
    print_provision_config_table(&resolved);

    if args.dry_run {
        println!("\n{} Dry run - not provisioning", style("i").cyan());
        return Ok(());
    }

    println!(
        "\n{} Provisioning {} via SSH\n",
        style("*").cyan(),
        style(&host).cyan()
    );

    // Create provider and provision
    let provider = Baremetal::new(&host, args.port);
    provider.provision(&tengu_config)?;

    // Set up Cloudflare Tunnel
    let tunnel_config = TunnelConfig {
        domain_platform: resolved.domain_platform.clone(),
        tunnel_name: "tengu".to_string(),
    };
    provider.setup_tunnel(&tunnel_config)?;

    // Print success
    print_baremetal_success(&tengu_config);

    Ok(())
}

/// Run show command
fn run_show(args: &ShowArgs, config: &Config) -> Result<()> {
    // Create a default TenguConfig from file config
    let tengu_config = TenguConfig::builder()
        .user(
            config
                .server
                .name
                .clone()
                .unwrap_or_else(|| "chi".to_string()),
        )
        .domain_platform(
            config
                .domains
                .platform
                .clone()
                .unwrap_or_else(|| "tengu.to".to_string()),
        )
        .domain_apps(
            config
                .domains
                .apps
                .clone()
                .unwrap_or_else(|| "tengu.host".to_string()),
        )
        .cf_api_key(
            config
                .cloudflare
                .api_key
                .clone()
                .unwrap_or_else(|| "<CF_API_KEY>".to_string()),
        )
        .cf_email(
            config
                .cloudflare
                .email
                .clone()
                .unwrap_or_else(|| "<CF_EMAIL>".to_string()),
        )
        .resend_api_key(
            config
                .resend
                .api_key
                .clone()
                .unwrap_or_else(|| "<RESEND_API_KEY>".to_string()),
        )
        .notify_email(
            config
                .notifications
                .email
                .clone()
                .unwrap_or_else(|| "admin@example.com".to_string()),
        )
        .ssh_keys(
            config
                .ssh
                .public_key
                .clone()
                .map(|k| vec![k])
                .unwrap_or_default(),
        )
        .release(
            config
                .server
                .release
                .clone()
                .unwrap_or_else(|| DEFAULT_RELEASE.to_string()),
        )
        .build();

    let manifest = Manifest::tengu(&tengu_config);

    match args.format {
        OutputFormat::CloudInit => {
            let renderer = CloudInitRenderer::new();
            let yaml = renderer.render_with_config(&manifest, &tengu_config)?;
            println!("{yaml}");
        }
        OutputFormat::Bash => {
            let renderer = BashRenderer::new().verbose(true).color(true);
            let script = renderer
                .render(&manifest)
                .map_err(|e| anyhow::anyhow!("Failed to render bash script: {e:?}"))?;
            println!("{script}");
        }
    }

    Ok(())
}

/// Print success for baremetal provisioning
fn print_baremetal_success(config: &TenguConfig) {
    println!();
    println!(
        "{}",
        style("+=======================================+")
            .green()
            .bold()
    );
    println!(
        "{}",
        style("|            SERVER READY!              |")
            .green()
            .bold()
    );
    println!(
        "{}",
        style("+=======================================+")
            .green()
            .bold()
    );
    println!();

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);

    table.add_row(vec![
        Cell::new("API").fg(Color::Cyan),
        Cell::new(format!("https://api.{}", config.domain_platform)),
    ]);
    table.add_row(vec![
        Cell::new("Docs").fg(Color::Cyan),
        Cell::new(format!("https://docs.{}", config.domain_platform)),
    ]);
    table.add_row(vec![
        Cell::new("Apps").fg(Color::Cyan),
        Cell::new(format!("https://<app>.{}", config.domain_apps)),
    ]);

    println!("{table}");
    println!();

    println!("{SPARKLE} Deployment complete!");
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
        style("║          TENGU PROVISIONING           ║")
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

/// Print config table for Hetzner flow (includes server type info)
fn print_hetzner_config_table(cfg: &ResolvedConfig, hetzner: &HetznerParams) -> Result<()> {
    let type_info = Hetzner::server_type_info(&hetzner.server_type)?;

    println!("\n{} Configuration\n", style("v").blue().bold());

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec![
        Cell::new("Setting").fg(Color::Cyan),
        Cell::new("Value").fg(Color::Cyan),
    ]);

    table.add_row(vec!["Name", &hetzner.name]);
    table.add_row(vec![
        "Type",
        &format!("{} ({})", hetzner.server_type, type_info),
    ]);
    table.add_row(vec!["Location", &hetzner.location]);
    table.add_row(vec!["Image", &hetzner.image]);
    table.add_row(vec!["Cloudflare", &cfg.cf_email]);
    table.add_row(vec![
        "Resend",
        &format!(
            "{}...",
            &cfg.resend_api_key[..12.min(cfg.resend_api_key.len())]
        ),
    ]);
    table.add_row(vec![
        "Domains",
        &format!("{}, {}", cfg.domain_platform, cfg.domain_apps),
    ]);
    table.add_row(vec!["Release", &cfg.release]);

    println!("{table}");
    Ok(())
}

/// Print config table for baremetal/SSH flow
fn print_provision_config_table(cfg: &ResolvedConfig) {
    println!("\n{} Configuration\n", style("v").blue().bold());

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec![
        Cell::new("Setting").fg(Color::Cyan),
        Cell::new("Value").fg(Color::Cyan),
    ]);

    table.add_row(vec!["Cloudflare", &cfg.cf_email]);
    table.add_row(vec![
        "Resend",
        &format!(
            "{}...",
            &cfg.resend_api_key[..12.min(cfg.resend_api_key.len())]
        ),
    ]);
    table.add_row(vec![
        "Domains",
        &format!("{}, {}", cfg.domain_platform, cfg.domain_apps),
    ]);
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
    context.insert("domain_ssh", &format!("ssh.{}", cfg.domain_platform));
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
    println!("\n{LOOKING_GLASS} Cloud-init preview:\n");
    // Show first 50 lines
    for line in content.lines().take(50) {
        println!("  {}", style(line).dim());
    }
    println!("  {}", style("... (truncated)").dim());
    Ok(())
}

fn wait_for_ssh(ip: &str) {
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
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-o",
                "LogLevel=ERROR",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "BatchMode=yes",
                &format!("chi@{ip}"),
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

    spinner.finish_with_message(format!("{CHECK} SSH ready"));
}

fn stream_cloud_init_logs(ip: &str) -> Result<()> {
    println!("\n{}", style("-".repeat(50)).dim());
    println!("{} Cloud-init progress:\n", style("v").cyan());

    let mut child = Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "LogLevel=ERROR",
            &format!("chi@{ip}"),
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
        for line in reader.lines().map_while(Result::ok) {
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

    let _ = child.wait();
    println!("\n{}", style("-".repeat(50)).dim());
    Ok(())
}

fn print_success(cfg: &ResolvedConfig, _ip: &str) {
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
        Cell::new(format!("ssh chi@ssh.{}", cfg.domain_platform)),
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

    println!("{SPARKLE} Deployment complete!");
}
