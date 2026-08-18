//! Device management CLI commands
//!
//! Provides commands to list, name, remove, and configure devices.

use std::io;

use clap::{Parser, Subcommand};

use crate::cli::Cli;
use crate::cloud::{CloudConfig, DeviceConfig};
use crate::ui::components::{Formatter, KeyValue, Renderable, StatusLine, Table};
use crate::ui::theme::ActiveTheme;

/// Device management commands
#[derive(Subcommand)]
pub enum DeviceCommands {
    /// List all registered devices
    List,

    /// Name the current device
    Name(DeviceNameArgs),

    /// Remove (deregister) a device
    Remove(DeviceRemoveArgs),

    /// Set SSH config for a device
    SshConfig(DeviceSshConfigArgs),

    /// Show current device info
    Info,

    /// Register this device with Cassy Cloud
    Register,
}

#[derive(Parser)]
pub struct DeviceNameArgs {
    /// Name to assign to the current device
    pub name: String,
}

#[derive(Parser)]
pub struct DeviceRemoveArgs {
    /// Device ID to remove
    pub id: String,
}

#[derive(Parser)]
pub struct DeviceSshConfigArgs {
    /// Device name or ID
    pub device: String,

    /// SSH host (e.g., user@hostname)
    pub host: String,
}

/// Execute a device subcommand
pub fn execute(cmd: &DeviceCommands, cli: &Cli) -> anyhow::Result<()> {
    match cmd {
        DeviceCommands::List => execute_list(cli),
        DeviceCommands::Name(args) => execute_name(args, cli),
        DeviceCommands::Remove(args) => execute_remove(args, cli),
        DeviceCommands::SshConfig(args) => execute_ssh_config(args, cli),
        DeviceCommands::Info => execute_info(cli),
        DeviceCommands::Register => execute_register(cli),
    }
}

/// Helper: load cloud config and ensure logged in
fn require_cloud_config() -> anyhow::Result<CloudConfig> {
    let config = CloudConfig::load().unwrap_or_default();
    if !config.is_logged_in() {
        anyhow::bail!("Not logged in. Run `cas login` first.");
    }
    Ok(config)
}

/// Helper: make authenticated API request
#[allow(clippy::result_large_err)]
fn api_get(config: &CloudConfig, path: &str) -> Result<ureq::Response, ureq::Error> {
    let url = format!("{}{}", config.endpoint, path);
    let token = config.token.as_deref().unwrap_or("");
    ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
}

#[allow(clippy::result_large_err)]
fn api_post(
    config: &CloudConfig,
    path: &str,
    body: &serde_json::Value,
) -> Result<ureq::Response, ureq::Error> {
    let url = format!("{}{}", config.endpoint, path);
    let token = config.token.as_deref().unwrap_or("");
    ureq::post(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_json(body)
}

#[allow(clippy::result_large_err)]
fn api_patch(
    config: &CloudConfig,
    path: &str,
    body: &serde_json::Value,
) -> Result<ureq::Response, ureq::Error> {
    let url = format!("{}{}", config.endpoint, path);
    let token = config.token.as_deref().unwrap_or("");
    ureq::patch(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_json(body)
}

#[allow(clippy::result_large_err)]
fn api_delete(config: &CloudConfig, path: &str) -> Result<ureq::Response, ureq::Error> {
    let url = format!("{}{}", config.endpoint, path);
    let token = config.token.as_deref().unwrap_or("");
    ureq::delete(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
}

// --- Command implementations ---

fn execute_list(cli: &Cli) -> anyhow::Result<()> {
    let config = require_cloud_config()?;

    let response = api_get(&config, "/api/devices").map_err(|e| match e {
        ureq::Error::Status(401, _) => anyhow::anyhow!("Authentication failed. Run `cas login`."),
        e => anyhow::anyhow!("Failed to list devices: {e}"),
    })?;

    let body: serde_json::Value = response.into_json()?;

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    let devices = body["devices"].as_array();
    let Some(devices) = devices else {
        StatusLine::info("No devices registered.").render(&mut fmt)?;
        return Ok(());
    };

    if devices.is_empty() {
        StatusLine::info("No devices registered.").render(&mut fmt)?;
        return Ok(());
    }

    // Check current device
    let current_device_id = DeviceConfig::load().ok().flatten().map(|d| d.device_id);

    let rows: Vec<Vec<String>> = devices
        .iter()
        .map(|device| {
            let id = device["id"].as_str().unwrap_or("-");
            let name = device["name"].as_str().unwrap_or("-");
            let os = device["os"].as_str().unwrap_or("-");
            let arch = device["arch"].as_str().unwrap_or("-");
            let status = device["status"].as_str().unwrap_or("offline");
            let last_seen = device["last_seen"].as_str().unwrap_or("-");

            let marker = if current_device_id.as_deref() == Some(id) {
                " *"
            } else {
                ""
            };

            vec![
                id.to_string(),
                name.to_string(),
                os.to_string(),
                arch.to_string(),
                status.to_string(),
                format!("{last_seen}{marker}"),
            ]
        })
        .collect();

    Table::new()
        .columns(&["ID", "NAME", "OS", "ARCH", "STATUS", "LAST SEEN"])
        .rows(rows)
        .render(&mut fmt)?;

    if current_device_id.is_some() {
        fmt.newline()?;
        fmt.write_muted("* = current device")?;
        fmt.newline()?;
    }

    Ok(())
}

fn execute_name(args: &DeviceNameArgs, cli: &Cli) -> anyhow::Result<()> {
    let config = require_cloud_config()?;

    let device_config = DeviceConfig::load()?.ok_or_else(|| {
        anyhow::anyhow!("Device not registered. Run `cas device register` first.")
    })?;

    let body = serde_json::json!({ "name": args.name });
    let response = api_patch(
        &config,
        &format!("/api/devices/{}", device_config.device_id),
        &body,
    )
    .map_err(|e| match e {
        ureq::Error::Status(404, _) => anyhow::anyhow!("Device not found in cloud."),
        e => anyhow::anyhow!("Failed to update device name: {e}"),
    })?;

    // Update local config too
    let mut device_config = device_config;
    device_config.name = Some(args.name.clone());
    device_config.save()?;

    if cli.json {
        let body: serde_json::Value = response.into_json()?;
        println!("{}", serde_json::to_string_pretty(&body)?);
    } else {
        let theme = ActiveTheme::default();
        let mut stdout = io::stdout();
        let mut fmt = Formatter::stdout(&mut stdout, theme);
        StatusLine::success(format!("Device named: {}", args.name)).render(&mut fmt)?;
    }

    Ok(())
}

fn execute_remove(args: &DeviceRemoveArgs, cli: &Cli) -> anyhow::Result<()> {
    let config = require_cloud_config()?;

    api_delete(&config, &format!("/api/devices/{}", args.id)).map_err(|e| match e {
        ureq::Error::Status(404, _) => anyhow::anyhow!("Device not found."),
        e => anyhow::anyhow!("Failed to remove device: {e}"),
    })?;

    // If removing current device, also delete local config
    if let Ok(Some(local)) = DeviceConfig::load() {
        if local.device_id == args.id {
            DeviceConfig::delete()?;
        }
    }

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "ok",
                "deleted": args.id
            }))?
        );
    } else {
        let theme = ActiveTheme::default();
        let mut stdout = io::stdout();
        let mut fmt = Formatter::stdout(&mut stdout, theme);
        StatusLine::success(format!("Device removed: {}", args.id)).render(&mut fmt)?;
    }

    Ok(())
}

fn execute_ssh_config(args: &DeviceSshConfigArgs, cli: &Cli) -> anyhow::Result<()> {
    let config = require_cloud_config()?;

    // First, find the device by name or ID
    let response = api_get(&config, "/api/devices")
        .map_err(|e| anyhow::anyhow!("Failed to list devices: {e}"))?;
    let body: serde_json::Value = response.into_json()?;
    let devices = body["devices"].as_array().unwrap_or(&vec![]).clone();

    let device = devices.iter().find(|d| {
        d["id"].as_str() == Some(&args.device) || d["name"].as_str() == Some(&args.device)
    });

    let Some(device) = device else {
        anyhow::bail!("Device '{}' not found.", args.device);
    };

    let device_id = device["id"].as_str().unwrap();

    let ssh_config = serde_json::json!({ "host": args.host });
    let body = serde_json::json!({ "ssh_config": ssh_config });

    api_patch(&config, &format!("/api/devices/{device_id}"), &body)
        .map_err(|e| anyhow::anyhow!("Failed to set SSH config: {e}"))?;

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "ok",
                "device": device_id,
                "ssh_config": ssh_config
            }))?
        );
    } else {
        let theme = ActiveTheme::default();
        let mut stdout = io::stdout();
        let mut fmt = Formatter::stdout(&mut stdout, theme);
        StatusLine::success(format!(
            "SSH config set for '{}': host={}",
            args.device, args.host
        ))
        .render(&mut fmt)?;
    }

    Ok(())
}

fn execute_info(cli: &Cli) -> anyhow::Result<()> {
    let local = DeviceConfig::load()?;
    let machine_hash = DeviceConfig::machine_hash();

    if cli.json {
        let info = serde_json::json!({
            "device_id": local.as_ref().map(|d| d.device_id.as_str()),
            "name": local.as_ref().and_then(|d| d.name.as_deref()),
            "registered_at": local.as_ref().map(|d| d.registered_at.as_str()),
            "machine_hash": machine_hash,
            "hostname": DeviceConfig::hostname(),
            "os": DeviceConfig::os(),
            "arch": DeviceConfig::arch(),
            "cas_version": DeviceConfig::cas_version(),
        });
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    match local {
        Some(device) => {
            let mut kv = KeyValue::new().add("Device ID", &device.device_id);
            if let Some(name) = &device.name {
                kv = kv.add("Name", name);
            }
            kv = kv.add("Registered", &device.registered_at);
            kv.render(&mut fmt)?;
        }
        None => {
            StatusLine::info("Not registered. Run `cas device register` to register.")
                .render(&mut fmt)?;
        }
    }

    let mut kv = KeyValue::new().add("Machine Hash", &machine_hash);
    if let Some(hostname) = DeviceConfig::hostname() {
        kv = kv.add("Hostname", &hostname);
    }
    kv = kv
        .add("OS", DeviceConfig::os())
        .add("Arch", DeviceConfig::arch())
        .add("Cassy Version", DeviceConfig::cas_version());
    kv.render(&mut fmt)?;

    Ok(())
}

fn execute_register(cli: &Cli) -> anyhow::Result<()> {
    let config = require_cloud_config()?;

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    // Check if already registered
    if let Ok(Some(existing)) = DeviceConfig::load() {
        if !cli.verbose {
            StatusLine::info(format!("Device already registered: {}", existing.device_id))
                .render(&mut fmt)?;
            if let Some(name) = &existing.name {
                fmt.field("Name", name)?;
            }
            fmt.info("Use `cas device name <name>` to rename.")?;
            return Ok(());
        }
    }

    let body = serde_json::json!({
        "machine_hash": DeviceConfig::machine_hash(),
        "hostname": DeviceConfig::hostname(),
        "os": DeviceConfig::os(),
        "arch": DeviceConfig::arch(),
        "cas_version": DeviceConfig::cas_version(),
    });

    let response = api_post(&config, "/api/devices", &body).map_err(|e| match e {
        ureq::Error::Status(401, _) => anyhow::anyhow!("Authentication failed. Run `cas login`."),
        e => anyhow::anyhow!("Failed to register device: {e}"),
    })?;

    let resp_body: serde_json::Value = response.into_json()?;
    let device_id = resp_body["device"]["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid response from server"))?;

    // Save locally
    let device_config = DeviceConfig {
        device_id: device_id.to_string(),
        name: resp_body["device"]["name"].as_str().map(String::from),
        registered_at: chrono::Utc::now().to_rfc3339(),
    };
    device_config.save()?;

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&resp_body)?);
    } else {
        StatusLine::success(format!("Device registered: {device_id}")).render(&mut fmt)?;
        fmt.info("Use `cas device name <name>` to give it a name.")?;
    }

    Ok(())
}
