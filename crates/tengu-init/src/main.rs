//! Tengu Init - Server Provisioning
//!
//! Provisions a server with Tengu PaaS installed.
//! - Default: connects to user@host via SSH and provisions
//! - `--hetzner`: creates a Hetzner VPS first, then provisions via SSH

mod providers;

use std::path::PathBuf;
use std::process::Command;
use std::{env, fs};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Color, Table, presets::UTF8_FULL_CONDENSED};
use console::{Emoji, style};
use dialoguer::{Input, Password};
use serde::{Deserialize, Serialize};
use tengu_provision::{BashRenderer, Manifest, Renderer, TenguConfig};

use providers::{Hetzner, SshProvider, TunnelConfig, hetzner::ServerParams};

static ROCKET: Emoji<'_, '_> = Emoji("🚀 ", "");
static SPARKLE: Emoji<'_, '_> = Emoji("✨ ", "");
static FOLDER: Emoji<'_, '_> = Emoji("📁 ", "");
static CHECK: Emoji<'_, '_> = Emoji("✅ ", "✓ ");

const DEFAULT_RELEASE: &str = "v0.1.0-22879bf";
const SSH_KEY_NAME: &str = "tengu-init";

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
    /// Admin username for Tengu (default: tengu)
    admin_user: Option<String>,
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
    about = "Provision Tengu PaaS servers via SSH"
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

    /// Admin username for Tengu (default: tengu)
    #[arg(long, short = 'u')]
    user: Option<String>,

    /// Config file path
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Enable UFW firewall configuration
    #[arg(long)]
    ufw: bool,

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
    /// Show generated provisioning script
    Show,
}

/// Resolved provisioning configuration (all credentials present)
struct ResolvedConfig {
    admin_user: String,
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

    // 10. Admin username
    let admin_user = args
        .user
        .clone()
        .or_else(|| config.server.admin_user.clone())
        .map_or_else(
            || {
                Input::<String>::new()
                    .with_prompt("Admin username")
                    .default("tengu".into())
                    .interact_text()
                    .context("Failed to read admin username")
            },
            Ok,
        )?;

    Ok(ResolvedConfig {
        admin_user,
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
    if let Some(Commands::Show) = &args.command {
        let file_config = load_config(args.config.as_ref())?;
        return run_show(&file_config);
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
            println!("{}", SshProvider::generate_removal_script());
            return Ok(());
        }

        let provider = SshProvider::new(host, args.port);
        provider.remove()?;

        return Ok(());
    }

    // Load config file
    let file_config = load_config(args.config.as_ref())?;

    // Resolve config (CLI > env > config > interactive > defaults)
    let resolved = resolve_config(&args, &file_config)?;

    // Build TenguConfig for provisioning
    let tengu_config = TenguConfig::builder()
        .user(&resolved.admin_user)
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
        .enable_ufw(args.ufw)
        .build();

    // Script-only mode (only for direct SSH)
    if args.script_only && !args.hetzner {
        let script = SshProvider::generate_script(&tengu_config)?;
        println!("{script}");
        return Ok(());
    }

    // Print banner
    print_banner();

    // Determine the host - either from args or create via Hetzner
    let (host, created_server) = if args.hetzner {
        let hetzner_params = resolve_hetzner_params(&args, &file_config);
        print_hetzner_config_table(&resolved, &hetzner_params)?;

        if args.dry_run {
            println!("\n{} Dry run - not creating server", style("i").cyan());
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

        // Ensure SSH key exists in Hetzner
        if !Hetzner::ssh_key_exists(SSH_KEY_NAME)? {
            println!("{} Creating SSH key in Hetzner...", style("*").cyan());
            Hetzner::create_ssh_key(SSH_KEY_NAME, &resolved.ssh_key)?;
        }

        // Create server (plain Ubuntu with SSH key)
        println!("\n{ROCKET} Creating server...");
        let params = ServerParams {
            name: &hetzner_params.name,
            server_type: &hetzner_params.server_type,
            image: &hetzner_params.image,
            location: &hetzner_params.location,
            ssh_key_name: SSH_KEY_NAME,
        };
        let ip = Hetzner::create_server(&params)?;

        println!("  {} IP: {}", style("->").dim(), style(&ip).cyan());

        // Remove old host key
        Hetzner::clear_host_key(&ip);

        // Host is root@ip (Hetzner default)
        (format!("root@{ip}"), true)
    } else {
        print_provision_config_table(&resolved);

        if args.dry_run {
            println!("\n{} Dry run - not provisioning", style("i").cyan());
            return Ok(());
        }

        (args.host.clone().unwrap(), false)
    };

    println!(
        "\n{} Provisioning {} via SSH\n",
        style("*").cyan(),
        style(&host).cyan()
    );

    // Create provider and provision
    let provider = SshProvider::new(&host, args.port);
    provider.provision(&tengu_config)?;

    // Set up Cloudflare Tunnel
    let tunnel_config = TunnelConfig {
        domain_platform: resolved.domain_platform.clone(),
        tunnel_name: "tengu".to_string(),
    };
    provider.setup_tunnel(&tunnel_config)?;

    // Print success
    if created_server {
        print_success(&resolved);
    } else {
        print_provision_success(&tengu_config);
    }

    Ok(())
}

/// Run show command - displays the generated provisioning script
fn run_show(config: &Config) -> Result<()> {
    // Create a default TenguConfig from file config
    let tengu_config = TenguConfig::builder()
        .user(
            config
                .server
                .admin_user
                .clone()
                .unwrap_or_else(|| "tengu".to_string()),
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
        .enable_ufw(false)
        .build();

    let manifest = Manifest::tengu(&tengu_config);
    let renderer = BashRenderer::new().verbose(true).color(true);
    let script = renderer
        .render(&manifest)
        .map_err(|e| anyhow::anyhow!("Failed to render bash script: {e:?}"))?;
    println!("{script}");

    Ok(())
}

/// Print success for SSH provisioning
fn print_provision_success(config: &TenguConfig) {
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
    table.add_row(vec!["Admin User", &cfg.admin_user]);
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

    table.add_row(vec!["Admin User", &cfg.admin_user]);
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

fn print_success(cfg: &ResolvedConfig) {
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
        Cell::new(format!("ssh {}@ssh.{}", cfg.admin_user, cfg.domain_platform)),
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
