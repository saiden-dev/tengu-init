//! Tengu Init - Server Provisioning
//!
//! Provisions a server with Tengu `PaaS` installed.
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
use tengu_provision::{BashRenderer, Manifest, Renderer, TenguConfig, TlsMode};

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
    mode: ModeConfig,
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
struct ModeConfig {
    /// TLS mode: "direct" or "cloudflare" (default: cloudflare)
    tls: Option<String>,
    /// ACME email for direct mode (defaults to notification email)
    acme_email: Option<String>,
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
    /// Scoped API token (preferred over global key)
    api_token: Option<String>,
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

    /// Skip confirmation prompt
    #[arg(long, short = 'y')]
    yes: bool,

    /// Generate script only, don't execute
    #[arg(long)]
    script_only: bool,

    /// Remove Tengu and all installed dependencies from the server
    #[arg(long)]
    remove: bool,

    /// Cloudflare API key (global)
    #[arg(long)]
    cf_api_key: Option<String>,

    /// Cloudflare email (with global key)
    #[arg(long)]
    cf_email: Option<String>,

    /// Cloudflare scoped API token (preferred over global key)
    #[arg(long)]
    cf_token: Option<String>,

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

    /// Use direct HTTPS (Let's Encrypt HTTP-01) instead of Cloudflare
    #[arg(long)]
    direct: bool,

    /// ACME email for direct mode (defaults to notification email)
    #[arg(long)]
    acme_email: Option<String>,

    /// Enable UFW firewall configuration
    #[arg(long)]
    ufw: bool,

    /// Path to local tengu .deb package (skips download)
    #[arg(long)]
    deb_path: Option<PathBuf>,

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
    tls_mode: TlsMode,
    resend_api_key: String,
    notify_email: String,
    ssh_key: String,
    release: String,
    /// Cloudflare credentials for DNS management (available in ALL modes if configured)
    cf_email: Option<String>,
    cf_api_key: Option<String>,
    /// Scoped API token (preferred over global key for DNS operations)
    cf_api_token: Option<String>,
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
    // Determine TLS mode: --direct flag > config file > interactive
    let is_direct = if args.direct {
        true
    } else if let Some(ref mode) = config.mode.tls {
        mode == "direct"
    } else {
        // Check if CF credentials are available anywhere — if not, default to prompting
        let has_cf = args.cf_email.is_some()
            || env::var("CF_EMAIL").is_ok()
            || config.cloudflare.email.is_some();
        if !has_cf && !args.yes {
            println!(
                "\n{}",
                style("--- Tengu Init \u{2014} TLS Mode ---").cyan().bold()
            );
            let selection = dialoguer::Select::new()
                .with_prompt("TLS mode")
                .items(&[
                    "Cloudflare (DNS-01 challenge + tunnel)",
                    "Direct HTTPS (Let's Encrypt HTTP-01, no Cloudflare)",
                ])
                .default(0)
                .interact()
                .context("Failed to read TLS mode")?;
            selection == 1
        } else {
            false
        }
    };

    let needs_interactive = !is_direct
        && args.cf_email.is_none()
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

    // TLS mode — resolve credentials based on mode
    let tls_mode = if is_direct {
        // Direct mode: just need an ACME email
        let acme_email = args
            .acme_email
            .clone()
            .or_else(|| config.mode.acme_email.clone())
            .or_else(|| args.notify_email.clone())
            .or_else(|| config.notifications.email.clone());

        // Will be resolved below after notify_email if still None
        TlsMode::Direct {
            acme_email: acme_email.unwrap_or_default(),
        }
    } else {
        // Cloudflare mode: need CF credentials
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

        // Cloudflare Tunnel auth - check for cert.pem
        if !cloudflared_cert_exists() {
            run_cloudflared_login()?;
        }

        TlsMode::Cloudflare {
            api_key: cf_api_key,
            email: cf_email,
        }
    };

    // Resend API key
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

    // Platform domain
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

    // Apps domain
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

    // SSH public key
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

    // Notification email (default: CF email in CF mode, or prompt in direct)
    let default_email = match &tls_mode {
        TlsMode::Cloudflare { email, .. } => email.clone(),
        TlsMode::Direct { acme_email } if !acme_email.is_empty() => acme_email.clone(),
        TlsMode::Direct { .. } => String::new(),
    };
    let notify_email = args
        .notify_email
        .clone()
        .or_else(|| config.notifications.email.clone())
        .map_or_else(
            || {
                let prompt = Input::<String>::new().with_prompt("Notification email");
                let prompt = if default_email.is_empty() {
                    prompt
                } else {
                    prompt.default(default_email.clone())
                };
                prompt
                    .interact_text()
                    .context("Failed to read notification email")
            },
            Ok,
        )?;

    // If direct mode and acme_email was empty, fill it from notify_email
    let tls_mode = match tls_mode {
        TlsMode::Direct { acme_email } if acme_email.is_empty() => TlsMode::Direct {
            acme_email: notify_email.clone(),
        },
        other => other,
    };

    // Tengu release
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

    // Admin username
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

    // CF credentials for DNS management — available in ALL modes.
    // Read from CLI args, env vars, config file, or TlsMode (if Cloudflare mode).
    let cf_api_token = args
        .cf_token
        .clone()
        .or_else(|| env::var("CF_API_TOKEN").ok())
        .or_else(|| config.cloudflare.api_token.clone());

    let (cf_email, cf_api_key) = match &tls_mode {
        TlsMode::Cloudflare { api_key, email } => {
            (Some(email.clone()), Some(api_key.clone()))
        }
        TlsMode::Direct { .. } => {
            // In direct mode, read CF creds silently from config/env (no interactive prompt)
            let email = args
                .cf_email
                .clone()
                .or_else(|| env::var("CF_EMAIL").ok())
                .or_else(|| config.cloudflare.email.clone());
            let key = args
                .cf_api_key
                .clone()
                .or_else(|| env::var("CF_API_KEY").ok())
                .or_else(|| config.cloudflare.api_key.clone());
            (email, key)
        }
    };

    Ok(ResolvedConfig {
        admin_user,
        domain_platform,
        domain_apps,
        tls_mode,
        resend_api_key,
        notify_email,
        ssh_key,
        release,
        cf_email,
        cf_api_key,
        cf_api_token,
    })
}

/// Resolve Hetzner-specific parameters
///
/// Default: cax41 (ARM64) in hel1 (Helsinki). Note that x86 `cpx*` types are
/// unavailable in EU since 2026-01-01. For x86, use `--location ash` or `--location hil`.
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
        .tls_mode(resolved.tls_mode.clone())
        .resend_api_key(&resolved.resend_api_key)
        .notify_email(&resolved.notify_email)
        .ssh_keys(if resolved.ssh_key.is_empty() {
            vec![]
        } else {
            vec![resolved.ssh_key.clone()]
        })
        .release(&resolved.release)
        .enable_ufw(args.ufw)
        .deb_path(args.deb_path.as_ref().map(|p| p.display().to_string()))
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
    // server_ip is Some(ip) when we created the server (for DNS update)
    let (host, server_ip) = if args.hetzner {
        let hetzner_params = resolve_hetzner_params(&args, &file_config);
        print_hetzner_config_table(&resolved, &hetzner_params)?;

        if !args.yes && !args.dry_run {
            let confirm = dialoguer::Confirm::new()
                .with_prompt(format!(
                    "Provision server {}? This will install Tengu PaaS and all dependencies",
                    hetzner_params.name
                ))
                .default(false)
                .interact()?;

            if !confirm {
                println!("Aborted.");
                return Ok(());
            }
        }

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

        // Ensure SSH key in Hetzner — try to create, fall back to finding existing
        let ssh_key_name: String;
        if Hetzner::ssh_key_exists(SSH_KEY_NAME)? {
            Hetzner::delete_ssh_key(SSH_KEY_NAME)?;
        }
        println!("{} Creating SSH key in Hetzner...", style("*").cyan());
        match Hetzner::create_ssh_key(SSH_KEY_NAME, &resolved.ssh_key) {
            Ok(()) => {
                ssh_key_name = SSH_KEY_NAME.to_string();
            }
            Err(e) => {
                let msg = format!("{e}");
                if msg.contains("uniqueness_error") || msg.contains("not unique") {
                    // Key content exists under another name — find it by fingerprint
                    ssh_key_name = Hetzner::find_key_name_by_content(&resolved.ssh_key)?
                        .unwrap_or_else(|| SSH_KEY_NAME.to_string());
                    println!(
                        "  {} SSH key exists as '{}', reusing",
                        style("*").dim(),
                        ssh_key_name
                    );
                } else {
                    return Err(e);
                }
            }
        }

        // Create server (plain Ubuntu with SSH key)
        println!("\n{ROCKET} Creating server...");
        let params = ServerParams {
            name: &hetzner_params.name,
            server_type: &hetzner_params.server_type,
            image: &hetzner_params.image,
            location: &hetzner_params.location,
            ssh_key_name: &ssh_key_name,
        };
        let ip = Hetzner::create_server(&params)?;

        println!("  {} IP: {}", style("->").dim(), style(&ip).cyan());

        // Remove old host key
        Hetzner::clear_host_key(&ip);

        // Host is root@ip (Hetzner default)
        (format!("root@{ip}"), Some(ip))
    } else {
        print_provision_config_table(&resolved);

        if !args.yes && !args.dry_run {
            let host_display = args.host.as_deref().unwrap_or("unknown");
            let confirm = dialoguer::Confirm::new()
                .with_prompt(format!(
                    "Provision server {host_display}? This will install Tengu PaaS and all dependencies"
                ))
                .default(false)
                .interact()?;

            if !confirm {
                println!("Aborted.");
                return Ok(());
            }
        }

        if args.dry_run {
            println!("\n{} Dry run - not provisioning", style("i").cyan());
            return Ok(());
        }

        (args.host.clone().unwrap(), None)
    };

    println!(
        "\n{} Provisioning {} via SSH\n",
        style("*").cyan(),
        style(&host).cyan()
    );

    // Create provider and provision
    let provider = SshProvider::new(&host, args.port);
    provider.provision(&tengu_config)?;

    // Post-provision: mode-dependent setup
    let has_cf_creds =
        resolved.cf_api_token.is_some() || (resolved.cf_api_key.is_some() && resolved.cf_email.is_some());

    match &resolved.tls_mode {
        TlsMode::Cloudflare { .. } => {
            // Set up Cloudflare Tunnel
            let tunnel_config = TunnelConfig {
                domain_platform: resolved.domain_platform.clone(),
                domain_apps: resolved.domain_apps.clone(),
                tunnel_name: "tengu".to_string(),
            };
            provider.setup_tunnel(&tunnel_config)?;

            // DNS records — via flarectl
            if let Some(ref ip) = server_ip {
                setup_dns_records(&resolved, ip)?;
            } else {
                // SSH mode with tunnel — CNAME to tunnel (keep legacy curl for now)
                if let (Some(key), Some(email)) = (&resolved.cf_api_key, &resolved.cf_email) {
                    update_wildcard_dns_tunnel(
                        email,
                        key,
                        &resolved.domain_apps,
                        &tunnel_config.tunnel_name,
                    )?;
                }
            }
        }
        TlsMode::Direct { .. } => {
            // Direct mode: create A records if CF credentials are available
            if has_cf_creds {
                if let Some(ref ip) = server_ip {
                    setup_dns_records(&resolved, ip)?;
                } else {
                    println!(
                        "\n{} No server IP available — DNS records must be created manually.",
                        style("!").yellow()
                    );
                }
            } else {
                // No CF credentials — print manual DNS reminder
                let ip_hint = server_ip.as_deref().unwrap_or("<your-server-ip>");
                println!(
                    "\n{} {}",
                    style("!").yellow().bold(),
                    style("DNS Configuration Required").yellow().bold()
                );
                println!("  Point these A records to {}:", style(ip_hint).cyan());
                println!("    api.{}", resolved.domain_platform);
                println!("    docs.{}", resolved.domain_platform);
                println!(
                    "    *.{}  (apps + git deploy via SSH)",
                    resolved.domain_apps
                );
                println!(
                    "\n  Caddy will automatically obtain Let's Encrypt certificates once DNS resolves."
                );
                println!(
                    "  {} Add [cloudflare] credentials to init.toml for automatic DNS setup.\n",
                    style("tip:").dim()
                );
            }
        }
    }

    // Print success
    if server_ip.is_some() {
        print_success(&resolved);
    } else {
        print_provision_success(&tengu_config);
    }

    Ok(())
}

/// Create or update a DNS A record via flarectl.
///
/// Supports two auth modes:
/// - Scoped API token: `CF_API_TOKEN` env var
/// - Global API key: `CF_API_KEY` + `CF_API_EMAIL` env vars
fn create_dns_record(
    config: &ResolvedConfig,
    zone: &str,
    name: &str,
    content: &str,
) -> Result<()> {
    let mut cmd = Command::new("flarectl");
    cmd.args([
        "dns",
        "create-or-update",
        "--zone",
        zone,
        "--type",
        "A",
        "--name",
        name,
        "--content",
        content,
        "--ttl",
        "1",
    ]);

    // Set auth env vars — token takes priority over global key
    if let Some(token) = &config.cf_api_token {
        cmd.env("CF_API_TOKEN", token);
    } else if let (Some(key), Some(email)) = (&config.cf_api_key, &config.cf_email) {
        cmd.env("CF_API_KEY", key);
        cmd.env("CF_API_EMAIL", email);
    } else {
        bail!("No Cloudflare credentials available for DNS");
    }

    let output = cmd.output().context("Failed to run flarectl")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("flarectl dns create-or-update failed: {stderr}");
    }

    println!("  {} {}.{} -> {}", style("v").green(), name, zone, content);
    Ok(())
}

/// Set up all required DNS A records for a Tengu instance.
///
/// Creates:
/// - `api.<domain-platform>` → VM IP
/// - `docs.<domain-platform>` → VM IP
/// - `*.<domain-apps>` → VM IP
fn setup_dns_records(config: &ResolvedConfig, ip: &str) -> Result<()> {
    println!(
        "\n{} Setting up DNS records (→ {})...",
        style("*").cyan(),
        style(ip).cyan()
    );

    // Platform records (api + docs) — zone is the platform domain
    create_dns_record(config, &config.domain_platform, "api", ip)?;
    create_dns_record(config, &config.domain_platform, "docs", ip)?;

    // Apps wildcard — zone is the apps domain
    create_dns_record(config, &config.domain_apps, "*", ip)?;

    println!(
        "  {} DNS records configured",
        style("✓").green().bold()
    );
    Ok(())
}

/// Update the wildcard DNS to a proxied CNAME pointing at the Cloudflare Tunnel.
///
/// Used when provisioning via SSH (not Hetzner) — traffic goes through the tunnel.
/// Replaces any existing A or CNAME record for `*.domain` with a proxied CNAME
/// to `{tunnel_id}.cfargotunnel.com`.
#[allow(clippy::too_many_lines)]
fn update_wildcard_dns_tunnel(
    cf_email: &str,
    cf_api_key: &str,
    domain: &str,
    tunnel_name: &str,
) -> Result<()> {
    println!(
        "\n{} Updating *.{} DNS to tunnel '{}'...",
        style("*").cyan(),
        domain,
        tunnel_name
    );

    // Step 1: Get zone ID
    let output = Command::new("curl")
        .args([
            "-sf",
            "-X",
            "GET",
            &format!("https://api.cloudflare.com/client/v4/zones?name={domain}"),
            "-H",
            &format!("X-Auth-Email: {cf_email}"),
            "-H",
            &format!("X-Auth-Key: {cf_api_key}"),
            "-H",
            "Content-Type: application/json",
        ])
        .output()
        .context("Failed to call Cloudflare API (zones)")?;

    let zones_json = String::from_utf8_lossy(&output.stdout);
    let zone_id = extract_json_field(&zones_json, "id")
        .context("Failed to extract zone ID from Cloudflare response")?;

    // Step 2: Get tunnel ID from cloudflared on the remote (already created by setup_tunnel)
    // We need the full UUID for the CNAME target
    let _tunnel_list = Command::new("ssh")
        .args([
            "-o",
            "ConnectTimeout=5",
            "root@localhost",
            &format!("cloudflared tunnel list -n {tunnel_name} -o json 2>/dev/null || true"),
        ])
        .output();

    // Fallback: use cloudflared locally if available, or construct from tunnel route dns output
    // For simplicity, use the tunnel name to route dns (cloudflared handles the CNAME)
    // But we're running from fuji, not the server. Use the CF API to find the tunnel.
    let tunnels_output = Command::new("curl")
        .args([
            "-sf",
            "-X",
            "GET",
            "https://api.cloudflare.com/client/v4/accounts",
            "-H",
            &format!("X-Auth-Email: {cf_email}"),
            "-H",
            &format!("X-Auth-Key: {cf_api_key}"),
            "-H",
            "Content-Type: application/json",
        ])
        .output()
        .context("Failed to get CF accounts")?;

    let accounts_json = String::from_utf8_lossy(&tunnels_output.stdout);
    let account_id =
        extract_json_field(&accounts_json, "id").context("Failed to extract account ID")?;

    // Get tunnel UUID
    let tunnels_output = Command::new("curl")
        .args([
            "-sf",
            "-X", "GET",
            &format!("https://api.cloudflare.com/client/v4/accounts/{account_id}/tunnels?name={tunnel_name}&is_deleted=false"),
            "-H", &format!("X-Auth-Email: {cf_email}"),
            "-H", &format!("X-Auth-Key: {cf_api_key}"),
            "-H", "Content-Type: application/json",
        ])
        .output()
        .context("Failed to get tunnel info")?;

    let tunnels_json = String::from_utf8_lossy(&tunnels_output.stdout);
    let tunnel_id =
        extract_json_field(&tunnels_json, "id").context("Failed to extract tunnel ID")?;

    let cname_target = format!("{tunnel_id}.cfargotunnel.com");

    // Step 3: Delete existing wildcard record (A or CNAME)
    for rtype in &["A", "CNAME"] {
        let output = Command::new("curl")
            .args([
                "-sf",
                "-X", "GET",
                &format!(
                    "https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records?type={rtype}&name=*.{domain}"
                ),
                "-H", &format!("X-Auth-Email: {cf_email}"),
                "-H", &format!("X-Auth-Key: {cf_api_key}"),
                "-H", "Content-Type: application/json",
            ])
            .output()?;

        let json = String::from_utf8_lossy(&output.stdout);
        if let Some(record_id) = extract_json_field(&json, "id") {
            Command::new("curl")
                .args([
                    "-sf",
                    "-X", "DELETE",
                    &format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records/{record_id}"),
                    "-H", &format!("X-Auth-Email: {cf_email}"),
                    "-H", &format!("X-Auth-Key: {cf_api_key}"),
                ])
                .output()?;
        }
    }

    // Step 4: Create proxied CNAME to tunnel
    let output = Command::new("curl")
        .args([
            "-sf",
            "-X", "POST",
            &format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records"),
            "-H", &format!("X-Auth-Email: {cf_email}"),
            "-H", &format!("X-Auth-Key: {cf_api_key}"),
            "-H", "Content-Type: application/json",
            "--data", &format!(
                r#"{{"type":"CNAME","name":"*.{domain}","content":"{cname_target}","proxied":true}}"#
            ),
        ])
        .output()
        .context("Failed to create wildcard CNAME record")?;

    if !output.status.success() {
        bail!("Failed to create wildcard DNS CNAME for tunnel");
    }

    println!(
        "  {} *.{} -> {} (proxied)",
        style("v").green(),
        domain,
        cname_target
    );

    Ok(())
}

/// Extract the first occurrence of a field value from a Cloudflare JSON response.
///
/// Looks for `"field":"value"` in the `result` array. This is a minimal parser
/// to avoid adding a JSON dependency -- the Cloudflare API responses are predictable.
fn extract_json_field(json: &str, field: &str) -> Option<String> {
    // Look for "result":[{..."field":"value"...
    let result_start = json.find("\"result\":[")?;
    let after_result = &json[result_start..];

    // Find the field within the first result object
    let pattern = format!("\"{field}\":\"");
    let field_start = after_result.find(&pattern)?;
    let value_start = field_start + pattern.len();
    let remaining = &after_result[value_start..];
    let value_end = remaining.find('"')?;

    Some(remaining[..value_end].to_string())
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
        .tls_mode(if config.mode.tls.as_deref() == Some("direct") {
            TlsMode::Direct {
                acme_email: config
                    .mode
                    .acme_email
                    .clone()
                    .or_else(|| config.notifications.email.clone())
                    .unwrap_or_else(|| "admin@example.com".to_string()),
            }
        } else {
            TlsMode::Cloudflare {
                api_key: config
                    .cloudflare
                    .api_key
                    .clone()
                    .unwrap_or_else(|| "<CF_API_KEY>".to_string()),
                email: config
                    .cloudflare
                    .email
                    .clone()
                    .unwrap_or_else(|| "<CF_EMAIL>".to_string()),
            }
        })
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

/// Add TLS mode rows to a config display table
fn add_tls_mode_rows(table: &mut Table, tls_mode: &TlsMode) {
    match tls_mode {
        TlsMode::Cloudflare { email, .. } => {
            table.add_row(vec!["TLS Mode", "Cloudflare (DNS-01)"]);
            table.add_row(vec!["CF Email", email]);
        }
        TlsMode::Direct { acme_email } => {
            table.add_row(vec!["TLS Mode", "Direct HTTPS (HTTP-01)"]);
            table.add_row(vec!["ACME Email", acme_email]);
        }
    }
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
    add_tls_mode_rows(&mut table, &cfg.tls_mode);
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
    add_tls_mode_rows(&mut table, &cfg.tls_mode);
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
        Cell::new(format!(
            "ssh {}@ssh.{}",
            cfg.admin_user, cfg.domain_platform
        )),
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
