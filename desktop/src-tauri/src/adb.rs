//! Minimal async wrapper around the bundled `adb` binary.
//! (A pure-Rust alternative is the `adb_client` crate; the CLI is used here because it
//! ships the Windows USB drivers glue and is battle-tested.)
//!
//! Every call has a hard timeout and kills the adb child on expiry: a device that is being
//! unplugged can leave `adb shell` hanging forever, which used to freeze the whole poll loop.
use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

/// Default timeout for one adb call (shell, forward, devices …).
pub const ADB_TIMEOUT: Duration = Duration::from_secs(15);
/// `adb install` of a ~10 MB APK over USB 2.0 + dex2oat on a slow tablet.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(240);

/// Build a Command that never flashes a console window on Windows.
pub fn hidden_command(bin: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(bin);
    #[cfg(windows)]
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}

/// Executable file name on this platform ("adb" -> "adb.exe" on Windows).
pub fn exe_name(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

/// Binary lookup: env override (ARGUS_ADB / ARGUS_LLAMA_SERVER) > bundled resources/ > next to the
/// executable > PATH. On Linux the bundled binaries may lose +x when unpacked: fix it best-effort.
pub fn resolve_bin(res_dir: Option<&Path>, name: &str) -> PathBuf {
    let env_key = format!("ARGUS_{}", name.to_uppercase().replace('-', "_"));
    if let Some(p) = std::env::var_os(&env_key).map(PathBuf::from).filter(|p| p.exists()) { return p; }
    let file = exe_name(name);
    let mut candidates: Vec<PathBuf> = vec![];
    if let Some(dir) = res_dir { candidates.push(dir.join(&file)); }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        candidates.push(dir.join("resources").join(&file));
        candidates.push(dir.join(&file));
    }
    for p in candidates {
        if p.is_file() {
            ensure_executable(&p);
            return p;
        }
    }
    PathBuf::from(file) // PATH lookup by the OS
}

#[cfg(unix)]
fn ensure_executable(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(md) = std::fs::metadata(p) {
        let mode = md.permissions().mode();
        if mode & 0o111 == 0 {
            let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode | 0o755));
        }
    }
}
#[cfg(not(unix))]
fn ensure_executable(_: &Path) {}

#[derive(Debug, Clone)]
pub struct AdbDevice {
    pub serial: String,
    pub state: String,           // "device" | "unauthorized" | "offline" | ...
    pub model: Option<String>,   // from `adb devices -l`
    /// adb transport id: changes on every re-plug, even when the serial is the same
    pub transport_id: Option<u64>,
}

#[derive(Clone)]
pub struct Adb { bin: PathBuf }

impl Adb {
    pub fn new(bin: PathBuf) -> Self { Self { bin } }

    pub async fn run(&self, args: &[&str]) -> Result<String> {
        self.run_timeout(args, ADB_TIMEOUT).await
    }

    pub async fn run_timeout(&self, args: &[&str], limit: Duration) -> Result<String> {
        let child = hidden_command(&self.bin).args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true) // a timed-out adb is killed when the future is dropped
            .output();
        let out = match tokio::time::timeout(limit, child).await {
            Err(_) => bail!("adb {}: нет ответа {} с (устройство отключено?)", short(args), limit.as_secs()),
            Ok(r) => r.map_err(|e| anyhow!("не удалось запустить adb ({}): {e}", self.bin.display()))?,
        };
        if !out.status.success() {
            bail!("adb {}: {}", short(args), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub async fn shell(&self, serial: &str, cmd: &str) -> Result<String> {
        self.run(&["-s", serial, "shell", cmd]).await
    }

    pub async fn devices(&self) -> Result<Vec<AdbDevice>> {
        let out = self.run_timeout(&["devices", "-l"], Duration::from_secs(8)).await?;
        Ok(parse_devices(&out))
    }

    /// Restart the adb server (wedged after a hard unplug on some Windows USB stacks).
    pub async fn restart_server(&self) {
        let _ = self.run_timeout(&["kill-server"], Duration::from_secs(8)).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Err(e) = self.run_timeout(&["start-server"], Duration::from_secs(20)).await { log::error!("adb start-server: {e}"); }
    }

    /// PC tcp:<local>  ->  device tcp:<remote>
    pub async fn forward(&self, serial: &str, local: u16, remote: u16) -> Result<()> {
        self.run(&["-s", serial, "forward", &format!("tcp:{local}"), &format!("tcp:{remote}")]).await.map(|_| ())
    }

    pub async fn remove_forward(&self, local: u16) -> Result<()> {
        self.run(&["forward", "--remove", &format!("tcp:{local}")]).await.map(|_| ())
    }

    /// Active forwards: (serial, local port, remote port).
    pub async fn forward_list(&self) -> Result<Vec<(String, u16, u16)>> {
        let out = self.run_timeout(&["forward", "--list"], Duration::from_secs(8)).await?;
        Ok(parse_forwards(&out))
    }

    pub async fn is_installed(&self, serial: &str, pkg: &str) -> Result<bool> {
        Ok(self.shell(serial, &format!("pm list packages --user 0 {pkg}")).await?
            .lines().any(|l| l.trim() == format!("package:{pkg}")))
    }

    /// (versionName, versionCode) of an installed package, None if missing/unknown.
    pub async fn package_version(&self, serial: &str, pkg: &str) -> Option<(String, Option<u64>)> {
        let out = self.shell(serial, &format!("dumpsys package {pkg} 2>/dev/null")).await.ok()?;
        let name = out.lines().find_map(|l| l.trim().strip_prefix("versionName=").map(|v| v.trim().to_string()))?;
        // "    versionCode=3 minSdk=26 targetSdk=34"
        let code = out.lines().find_map(|l| l.trim().strip_prefix("versionCode="))
            .and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse().ok());
        Some((name, code))
    }

    pub async fn install(&self, serial: &str, apk: &Path) -> Result<()> {
        let apk = apk.to_string_lossy();
        let out = self.run_timeout(&["-s", serial, "install", "--user", "0", "-r", "-g", &apk], INSTALL_TIMEOUT).await?;
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

fn short(args: &[&str]) -> String {
    // "-s SERIAL shell <long command>" -> keep it readable in the UI
    let s = args.join(" ");
    if s.chars().count() > 80 { format!("{}…", s.chars().take(80).collect::<String>()) } else { s }
}

pub fn parse_devices(out: &str) -> Vec<AdbDevice> {
    out.lines().filter(|l| !l.starts_with("List of devices") && !l.starts_with('*')).filter_map(|line| {
        let mut parts = line.split_whitespace();
        let serial = parts.next()?.to_string();
        let state = parts.next()?.to_string();
        let rest: Vec<&str> = parts.collect();
        let model = rest.iter().find_map(|p| p.strip_prefix("model:")).map(|m| m.replace('_', " "));
        let transport_id = rest.iter().find_map(|p| p.strip_prefix("transport_id:")).and_then(|v| v.parse().ok());
        Some(AdbDevice { serial, state, model, transport_id })
    }).collect()
}

pub fn parse_forwards(out: &str) -> Vec<(String, u16, u16)> {
    out.lines().filter_map(|l| {
        let p: Vec<&str> = l.split_whitespace().collect();
        if p.len() < 3 { return None; }
        let local = p[1].strip_prefix("tcp:")?.parse().ok()?;
        let remote = p[2].strip_prefix("tcp:")?.parse().ok()?;
        Some((p[0].to_string(), local, remote))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn devices_and_forwards() {
        let d = parse_devices("List of devices attached\nR52T1234 device usb:1-1 product:gta4l model:SM_T505 device:gta4l transport_id:7\nABC unauthorized usb:1-2 transport_id:9\nXYZ offline\n\n");
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].model.as_deref(), Some("SM T505"));
        assert_eq!(d[0].transport_id, Some(7));
        assert_eq!(d[1].state, "unauthorized");
        assert_eq!(d[2].transport_id, None);
        let f = parse_forwards("R52T1234 tcp:50052 tcp:50052\nABC tcp:50053 tcp:50052\nX localabstract:foo tcp:1\n");
        assert_eq!(f, vec![("R52T1234".into(), 50052, 50052), ("ABC".into(), 50053, 50052)]);
    }
}
