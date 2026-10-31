//! Minimal async wrapper around the bundled `adb` binary.
//! (A pure-Rust alternative is the `adb_client` crate; the CLI is used here because it
//! ships the Windows USB drivers glue and is battle-tested.)
use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Build a Command that never flashes a console window on Windows.
pub fn hidden_command(bin: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(bin);
    #[cfg(windows)]
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}

/// Prefer the binary bundled in resources/, else fall back to PATH.
pub fn resolve_bin(res_dir: Option<&Path>, name: &str) -> PathBuf {
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    if let Some(dir) = res_dir {
        let p = dir.join(&file);
        if p.exists() { return p; }
    }
    PathBuf::from(file)
}

#[derive(Debug, Clone)]
pub struct AdbDevice {
    pub serial: String,
    pub state: String,           // "device" | "unauthorized" | "offline" | ...
    pub model: Option<String>,   // from `adb devices -l`
}

#[derive(Clone)]
pub struct Adb { bin: PathBuf }

impl Adb {
    pub fn new(bin: PathBuf) -> Self { Self { bin } }

    pub async fn run(&self, args: &[&str]) -> Result<String> {
        let out = hidden_command(&self.bin).args(args).output().await
            .map_err(|e| anyhow!("failed to run adb ({}): {e}", self.bin.display()))?;
        if !out.status.success() {
            bail!("adb {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub async fn shell(&self, serial: &str, cmd: &str) -> Result<String> {
        self.run(&["-s", serial, "shell", cmd]).await
    }

    pub async fn devices(&self) -> Result<Vec<AdbDevice>> {
        let out = self.run(&["devices", "-l"]).await?;
        Ok(out.lines().skip(1).filter_map(|line| {
            let mut parts = line.split_whitespace();
            let serial = parts.next()?.to_string();
            let state = parts.next()?.to_string();
            let model = parts.find_map(|p| p.strip_prefix("model:")).map(|m| m.replace('_', " "));
            Some(AdbDevice { serial, state, model })
        }).collect())
    }

    /// PC tcp:<local>  ->  device tcp:<remote>
    pub async fn forward(&self, serial: &str, local: u16, remote: u16) -> Result<()> {
        self.run(&["-s", serial, "forward", &format!("tcp:{local}"), &format!("tcp:{remote}")]).await.map(|_| ())
    }

    pub async fn remove_forward(&self, local: u16) -> Result<()> {
        self.run(&["forward", "--remove", &format!("tcp:{local}")]).await.map(|_| ())
    }

    pub async fn is_installed(&self, serial: &str, pkg: &str) -> Result<bool> {
        Ok(self.shell(serial, &format!("pm list packages {pkg}")).await?
            .lines().any(|l| l.trim() == format!("package:{pkg}")))
    }

    /// (versionName, versionCode) of an installed package, None if missing/unknown.
    pub async fn package_version(&self, serial: &str, pkg: &str) -> Option<(String, Option<u64>)> {
        let out = self.shell(serial, &format!("dumpsys package {pkg}")).await.ok()?;
        let name = out.lines().find_map(|l| l.trim().strip_prefix("versionName=").map(|v| v.trim().to_string()))?;
        // "    versionCode=3 minSdk=26 targetSdk=34"
        let code = out.lines().find_map(|l| l.trim().strip_prefix("versionCode="))
            .and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse().ok());
        Some((name, code))
    }

    pub async fn install(&self, serial: &str, apk: &Path) -> Result<()> {
        let apk = apk.to_string_lossy();
        let out = self.run(&["-s", serial, "install", "-r", "-g", &apk]).await?;
        if !out.contains("Success") { bail!("install failed: {}", out.trim()); }
        Ok(())
    }

    pub async fn getprop(&self, serial: &str, key: &str) -> String {
        self.shell(serial, &format!("getprop {key}")).await.map(|s| s.trim().to_string()).unwrap_or_default()
    }

    /// (MemTotal, MemAvailable) in kB.
    pub async fn meminfo(&self, serial: &str) -> Result<(u64, u64)> {
        let out = self.shell(serial, "cat /proc/meminfo").await?;
        let field = |name: &str| out.lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok());
        Ok((field("MemTotal:").ok_or_else(|| anyhow!("no MemTotal"))?,
            field("MemAvailable:").unwrap_or(0)))
    }

    /// (idle, total) jiffies of the aggregate CPU line. Readable from the adb shell domain.
    pub async fn cpu_sample(&self, serial: &str) -> Result<(u64, u64)> {
        let out = self.shell(serial, "head -n1 /proc/stat").await?;
        let v: Vec<u64> = out.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
        if v.len() < 4 { bail!("unexpected /proc/stat") }
        let idle = v[3] + v.get(4).copied().unwrap_or(0); // idle + iowait
        Ok((idle, v.iter().sum()))
    }
}
