//! Wi-Fi node beacon (UDP :50053) format. Kept free of tauri so it can be unit-tested anywhere.
use serde::Deserialize;
use std::net::IpAddr;
use std::time::Instant;

/// rpc-server port when the beacon does not say
pub const DEFAULT_PORT: u16 = 50052;

/// A node heard on UDP :50053 (Argus Node Wi-Fi mode, argus-node-linux.sh).
#[derive(Debug, Clone)]
pub struct Discovered {
    pub host: String,
    pub port: u16,
    pub name: String,
    pub ram_mb: u64,
    pub free_mb: u64,
    pub variant: Option<String>,
    pub last_seen: Instant,
    /// UDP source port of the beacon: the node listens there for the PC's {"argus":"host",...} reply
    pub src_port: u16,
    /// Argus Node versionName / versionCode (beacon "vn"/"vc", node 0.3.1+)
    pub vn: Option<String>,
    pub vc: Option<u64>,
    /// 0.3.2: autobench result of the node app (GB/s), and its «Помощник» status
    pub bench: Option<f32>,
    pub helper: Option<BeaconHelper>,
    /// optional thermal state (node 0.3.3+): °C and throttling flag
    pub temp_c: Option<f32>,
    pub throttled: Option<bool>,
    /// node 0.3.3+: `roles` map {helper,voice,library:{state,port,pct,threads,model|zim,error}} (Roles.json() on the phone)
    pub roles: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct BeaconHelper {
    #[serde(default)] pub state: String,
    #[serde(default)] pub model: Option<String>,
    #[serde(default)] pub pct: Option<u32>,
    #[serde(default)] pub error: Option<String>,
}
impl Discovered {
    pub fn endpoint(&self) -> String { format!("{}:{}", self.host, self.port) }
    pub fn id(&self) -> String { format!("lan:{}:{}", self.host, self.port) }
}

#[derive(Deserialize)]
struct Beacon {
    argus: String,
    #[serde(default)] v: Option<u32>,
    #[serde(default)] name: Option<String>,
    #[serde(default)] port: Option<u16>,
    #[serde(default)] ram_mb: Option<u64>,
    #[serde(default)] free_mb: Option<u64>,
    #[serde(default)] variant: Option<String>,
    #[serde(default)] vn: Option<String>,
    #[serde(default)] vc: Option<u64>,
    #[serde(default)] bench: Option<f32>,
    #[serde(default)] helper: Option<BeaconHelper>,
    #[serde(default)] temp_c: Option<f32>,
    #[serde(default)] throttled: Option<bool>,
    #[serde(default)] roles: Option<serde_json::Value>,
}

/// {"argus":"node","v":1,"name":"<model>","port":50052,"ram_mb":N,"free_mb":N,"variant":"dotprod","vn":"0.3.1","vc":5}
/// ("v" is the beacon protocol version, "vn"/"vc" the app version — added in 0.3.1, optional)
pub fn parse_beacon(bytes: &[u8], src: IpAddr) -> Option<Discovered> {
    let b: Beacon = serde_json::from_slice(bytes).ok()?;
    if b.argus != "node" || b.v.unwrap_or(1) != 1 { return None; }
    let host = match src { IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(src), x => x }.to_string();
    let name: String = b.name.unwrap_or_default().chars().filter(|c| !c.is_control()).take(48).collect();
    Some(Discovered {
        name: if name.trim().is_empty() { host.clone() } else { name.trim().to_string() },
        host, port: b.port.filter(|p| *p != 0).unwrap_or(DEFAULT_PORT),
        ram_mb: b.ram_mb.unwrap_or(0), free_mb: b.free_mb.unwrap_or(0),
        variant: b.variant.map(|v| v.chars().take(24).collect()), last_seen: Instant::now(),
        src_port: 0, vn: b.vn.map(|v| v.chars().filter(|c| !c.is_control()).take(24).collect()), vc: b.vc,
        bench: b.bench.filter(|g| g.is_finite() && *g > 0.0 && *g < 1000.0),
        temp_c: b.temp_c.filter(|t| t.is_finite() && *t > -40.0 && *t < 150.0), throttled: b.throttled,
        roles: b.roles.filter(|r| r.is_object()),
        helper: b.helper.map(|mut h| { h.error = h.error.map(|e| e.chars().filter(|c| !c.is_control()).take(200).collect()); h }),
    })
}

/// Unicast reply to a node beacon: where to fetch node.apk and which versionCode this PC bundles.
/// 0.3.2: `helper` = what the node should run as «Помощник» ({"file","path","size","port"}), `false` = stop,
/// `null` = no change (model not on the PC yet).
pub fn host_reply(update_port: u16, node_vc: Option<u64>, force_update: bool, helper: Option<serde_json::Value>) -> Vec<u8> {
    let mut j = serde_json::json!({ "argus": "host", "v": 1, "update_port": update_port, "node_vc": node_vc.unwrap_or(0) });
    if force_update { j["update"] = serde_json::Value::Bool(true); }
    if let Some(h) = helper.filter(|h| !h.is_null()) { j["helper"] = h; }
    j.to_string().into_bytes()
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn beacon() {
        let ip: IpAddr = "192.168.1.42".parse().unwrap();
        let d = parse_beacon(br#"{"argus":"node","v":1,"name":"SM-T505","port":50052,"ram_mb":3700,"free_mb":2100,"variant":"dotprod"}"#, ip).unwrap();
        assert_eq!(d.endpoint(), "192.168.1.42:50052");
        assert_eq!(d.id(), "lan:192.168.1.42:50052");
        assert_eq!((d.ram_mb, d.free_mb), (3700, 2100));
        assert_eq!(d.variant.as_deref(), Some("dotprod"));
        assert!(parse_beacon(br#"{"argus":"other"}"#, ip).is_none());
        assert!(parse_beacon(br#"{"argus":"node","v":2}"#, ip).is_none());
        assert!(parse_beacon(b"garbage", ip).is_none());
        let d = parse_beacon(br#"{"argus":"node"}"#, ip).unwrap();
        assert_eq!((d.port, d.name.as_str()), (50052, "192.168.1.42"));
        assert_eq!(d.vc, None);
        let d = parse_beacon(br#"{"argus":"node","v":1,"vn":"0.3.1","vc":5}"#, ip).unwrap();
        assert_eq!((d.vn.as_deref(), d.vc), (Some("0.3.1"), Some(5)));
        let r: serde_json::Value = serde_json::from_slice(&host_reply(50055, Some(5), false, None)).unwrap();
        assert_eq!((r["argus"].as_str(), r["node_vc"].as_u64(), r.get("update").is_none()), (Some("host"), Some(5), true));
        let d = parse_beacon(br#"{"argus":"node","v":1,"bench":12.5,"helper":{"state":"downloading","pct":40,"model":"x.gguf"}}"#, ip).unwrap();
        assert_eq!(d.bench, Some(12.5));
        assert_eq!(d.helper.as_ref().map(|h| (h.state.as_str(), h.pct)), Some(("downloading", Some(40))));
        let r: serde_json::Value = serde_json::from_slice(&host_reply(50055, Some(6), false, Some(serde_json::Value::Bool(false)))).unwrap();
        assert_eq!(r["helper"], serde_json::Value::Bool(false));
        let d = parse_beacon(br#"{"argus":"node","v":1,"roles":{"voice":{"state":"ready","port":50061,"threads":2},"library":{"state":"idle","port":50063}}}"#, ip).unwrap();
        assert_eq!(d.roles.as_ref().and_then(|r| r["voice"]["state"].as_str()), Some("ready"));
        assert!(parse_beacon(br#"{"argus":"node","v":1,"roles":5}"#, ip).unwrap().roles.is_none());
    }
}
