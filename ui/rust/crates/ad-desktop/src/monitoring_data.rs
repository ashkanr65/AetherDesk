//! Validated monitor configuration and bounded output parsing; no network/UI work.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::LazyLock};

pub const MAX_OUTPUT: usize = 256 * 1024;
pub const SYSTEM_SCRIPT: &str = include_str!("../../../../../tools/rust-build-kit/monitor_host.sh");

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Format {
    Text,
    Value,
    Table,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Kind {
    FlexNet { executable: String, server: String },
    Custom { command: String, format: Format },
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Filters {
    pub feature: String,
    pub user: String,
    pub machine: String,
    pub search: String,
    pub group_users: bool,
    pub show_unused: bool,
    /// One exact feature=display label per line. Never used as executable code.
    pub aliases: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Monitor {
    pub id: String,
    pub profile: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub show_in_bar: bool,
    pub interval: u32,
    pub kind: Kind,
    #[serde(default)]
    pub filters: Filters,
}
impl Monitor {
    pub fn new(profile: String, licence: bool) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            profile,
            name: if licence {
                "Licence usage"
            } else {
                "Custom check"
            }
            .into(),
            enabled: true,
            show_in_bar: false,
            interval: if licence { 60 } else { 30 },
            kind: if licence {
                Kind::FlexNet {
                    executable: "lmutil".into(),
                    server: String::new(),
                }
            } else {
                Kind::Custom {
                    command: String::new(),
                    format: Format::Text,
                }
            },
            filters: Filters::default(),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if uuid::Uuid::parse_str(&self.id).is_err()
            || self.profile.is_empty()
            || self.profile.len() > 256
            || self.name.trim().is_empty()
            || self.name.len() > 100
        {
            return Err("Enter a monitor name (up to 100 characters).".into());
        }
        let minimum = if matches!(self.kind, Kind::FlexNet { .. }) {
            30
        } else {
            10
        };
        if !(minimum..=3600).contains(&self.interval) {
            return Err(format!("Refresh must be {minimum}–3600 seconds."));
        }
        match &self.kind {
            Kind::FlexNet { executable, server } => {
                if [executable, server].iter().any(|s| {
                    s.trim().is_empty() || s.len() > 1024 || s.contains(['\0', '\n', '\r'])
                }) {
                    return Err("Enter the lmutil or lmstat path and licence server (port@host or licence-file path).".into());
                }
            }
            Kind::Custom { command, .. } => {
                if command.trim().is_empty() || command.len() > 16384 || command.contains('\0') {
                    return Err("Enter a command or script of up to 16 KiB.".into());
                }
            }
        }
        if [
            &self.filters.feature,
            &self.filters.user,
            &self.filters.machine,
            &self.filters.search,
        ]
        .iter()
        .any(|v| v.len() > 256)
            || self.filters.aliases.len() > 8192
            || self.filters.aliases.lines().count() > 64
            || self
                .filters
                .aliases
                .lines()
                .any(|line| line.chars().count() > 512)
        {
            return Err("Monitor filters are too long.".into());
        }
        Ok(())
    }
    pub fn key(&self) -> String {
        serde_json::to_string(&self.kind).expect("serializable monitor kind")
    }
    pub fn command(&self) -> Result<String, String> {
        self.validate()?;
        Ok(match &self.kind {
            Kind::FlexNet { executable, server } => {
                let standalone = executable
                    .rsplit('/')
                    .next()
                    .unwrap_or(executable)
                    .eq_ignore_ascii_case("lmstat");
                format!(
                    "export LC_ALL=C; exec {}{} -c {} -a -t 5",
                    quote(executable),
                    if standalone { "" } else { " lmstat" },
                    quote(server)
                )
            }
            Kind::Custom { command, .. } => command.clone(),
        })
    }
}
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
pub fn system_command(slow: bool) -> String {
    format!(
        "exec sh -c {} sh {}",
        quote(SYSTEM_SCRIPT),
        if slow { "slow" } else { "fast" }
    )
}
pub fn homes_command() -> String {
    format!("exec sh -c {} sh homes", quote(SYSTEM_SCRIPT))
}
#[derive(Clone, Debug)]
pub struct HomeUsage {
    pub path: String,
    pub owner: String,
    pub kib: Option<u64>,
}
#[derive(Clone, Debug, Default)]
pub struct System {
    pub host: String,
    pub home: String,
    pub homes: Vec<HomeUsage>,
    pub homes_error: Option<String>,
    pub cpu: Option<(u64, u64)>,
    pub memory: Option<(u64, u64, u64, u64)>,
    pub uptime: Option<u64>,
    pub network: BTreeMap<String, (u64, u64)>,
    pub users: Vec<(String, String, String)>,
    pub disks: Vec<(String, u64, u64, u64)>,
    pub users_error: Option<String>,
    pub disks_error: Option<String>,
}
fn text_limit(text: &str) -> Result<(), String> {
    if text.len() > MAX_OUTPUT || text.contains('\0') {
        Err("Monitor output is invalid or exceeds 256 KiB.".into())
    } else {
        Ok(())
    }
}
pub fn parse_system(text: &str, slow: bool) -> Result<System, String> {
    text_limit(text)?;
    if text.lines().next() != Some("AETHER_MONITOR_V1") {
        return Err(
            "This monitor requires a Linux server with /proc and standard system utilities.".into(),
        );
    }
    let mut sample = System::default();
    let mut section = "";
    for line in text.lines().skip(1) {
        match line {
            "USERS_BEGIN" => {
                section = "users";
                continue;
            }
            "USERS_END" => {
                section = "";
                continue;
            }
            "DISKS_BEGIN" => {
                section = "disks";
                continue;
            }
            "DISKS_END" => {
                section = "";
                continue;
            }
            _ => {}
        }
        let fields: Vec<_> = line.split('\t').collect();
        let number = |i: usize| fields.get(i).and_then(|v| v.parse::<u64>().ok());
        match fields.first().copied() {
            Some("ERR") => match fields.get(1).copied() {
                Some("homes") => sample.homes_error = Some("Some folders were unreadable, exceeded the scan limit or timed out; sizes shown are lower bounds, not complete totals.".into()),
                Some("users") => sample.users_error = Some("Login records unavailable".into()),
                Some("disks") => {
                    sample.disks_error = Some("Local filesystem statistics unavailable".into())
                }
                _ => {}
            },
            Some("HOME") => sample.home = fields.get(1).unwrap_or(&"").chars().take(1024).collect(),
            Some("HOME_DIR") if sample.homes.len() < 64 => {
                if let (Some(owner), Some(path)) = (fields.get(1), fields.get(2)) {
                    if path.len() <= 4096 && owner.len() <= 128 {
                        sample.homes.push(HomeUsage { path: (*path).into(), owner: (*owner).into(), kib: None });
                    }
                }
            }
            Some("HOME_SIZE") => {
                if let (Some(size), Some(path)) = (number(1), fields.get(2)) {
                    if let Some(home) = sample.homes.iter_mut().find(|h| h.path == *path) {
                        home.kib = Some(size);
                    }
                }
            }
            Some("HOST") => sample.host = fields.get(1).unwrap_or(&"").chars().take(256).collect(),
            Some("CPU") => {
                sample.cpu = number(1)
                    .zip(number(2))
                    .filter(|(total, idle)| total >= idle)
            }
            Some("MEM") => {
                sample.memory = number(1)
                    .zip(number(2))
                    .zip(number(3).zip(number(4)))
                    .filter(|((t, a), (s, f))| *t > 0 && a <= t && f <= s)
                    .map(|((t, a), (s, f))| (t, a, s, f))
            }
            Some("UP") => sample.uptime = number(1),
            Some("NET") if sample.network.len() < 64 => {
                if let (Some(name), Some(rx), Some(tx)) = (fields.get(1), number(2), number(3)) {
                    if name.len() <= 64 {
                        sample.network.insert((*name).into(), (rx, tx));
                    }
                }
            }
            _ if section == "users" => {
                if sample.users.len() >= 512 {
                    sample.users_error =
                        Some("Login record limit reached; counts unavailable.".into());
                    continue;
                }
                let mut f = line.split_whitespace();
                if let (Some(user), Some(tty), Some(date), Some(time)) =
                    (f.next(), f.next(), f.next(), f.next())
                {
                    sample.users.push((
                        user.chars().take(128).collect(),
                        tty.chars().take(128).collect(),
                        format!("{date} {time}"),
                    ));
                }
            }
            _ if section == "disks" => {
                if sample.disks.len() >= 128 {
                    sample.disks_error = Some("Showing the first 128 local filesystems.".into());
                    continue;
                }
                let f: Vec<_> = line.split_whitespace().collect();
                if f.len() >= 6 {
                    if let (Ok(total), Ok(used), Ok(available)) =
                        (f[1].parse(), f[2].parse(), f[3].parse())
                    {
                        sample
                            .disks
                            .push((f[5..].join(" "), total, used, available));
                    }
                }
            }
            _ => {}
        }
    }
    if !slow && (sample.cpu.is_none() || sample.memory.is_none()) {
        return Err("CPU or memory statistics unavailable on this server.".into());
    }
    if slow && sample.disks.is_empty() && sample.disks_error.is_none() {
        sample.disks_error = Some("No local filesystem statistics returned".into());
    }
    Ok(sample)
}
pub fn cpu_percent(previous: (u64, u64), current: (u64, u64)) -> Option<f32> {
    let total = current.0.checked_sub(previous.0)?;
    let idle = current.1.checked_sub(previous.1)?;
    (total > 0 && idle <= total).then(|| 100.0 * (total - idle) as f32 / total as f32)
}
#[derive(Clone, Debug)]
pub struct Checkout {
    pub user: String,
    pub machine: String,
    pub display: String,
    pub since: String,
    pub count: u64,
}
#[derive(Clone, Debug)]
pub struct Feature {
    pub name: String,
    pub total: u64,
    pub used: u64,
    pub users: Vec<Checkout>,
}
static HEADER: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"^Users of (.+):\s*\(Total of (\d+) licenses? issued;\s*Total of (\d+) licenses? in use\)",
    )
    .unwrap()
});
pub fn parse_licences(text: &str) -> Result<Vec<Feature>, String> {
    text_limit(text)?;
    let mut result: Vec<Feature> = Vec::new();
    for line in text.lines() {
        if let Some(c) = HEADER.captures(line.trim()) {
            if c[1].len() > 512 {
                return Err("Licence feature name is too long.".into());
            }
            if result.len() >= 2048 {
                return Err("Too many licence features; narrow the source report.".into());
            }
            result.push(Feature {
                name: c[1].into(),
                total: c[2].parse().map_err(|_| "Invalid licence total")?,
                used: c[3].parse().map_err(|_| "Invalid licence usage")?,
                users: vec![],
            });
        } else if let Some(feature) = result.last_mut() {
            // Checkout lines include a version and start time; vendor/feature
            // descriptions must not become invented users. Keep host/display.
            if let Some((prefix, since)) = line.trim().split_once(", start ") {
                if prefix.contains("(v") {
                    let f: Vec<_> = prefix.split_whitespace().collect();
                    if f.len() >= 4 && feature.users.len() < 512 {
                        let (since, count) = since
                            .rsplit_once(", ")
                            .and_then(|(date, seats)| {
                                let mut words = seats.split_whitespace();
                                let count = words.next()?.parse::<u64>().ok()?;
                                let unit = words.next()?;
                                (count > 0
                                    && matches!(unit, "license" | "licenses")
                                    && words.next().is_none())
                                .then_some((date, count))
                            })
                            .unwrap_or((since, 1));
                        feature.users.push(Checkout {
                            user: f[0].into(),
                            machine: f[1].into(),
                            display: f[2].into(),
                            since: since.chars().take(256).collect(),
                            count,
                        });
                    }
                }
            }
        }
    }
    if result.is_empty() {
        return Err("No supported FlexNet feature records returned. Check the server, lmutil/lmstat path and raw output.".into());
    }
    Ok(result)
}
fn contains(value: &str, query: &str) -> bool {
    value.to_lowercase().contains(&query.trim().to_lowercase())
}
impl Filters {
    pub fn label(&self, feature: &str) -> String {
        self.aliases
            .lines()
            .filter_map(|l| l.split_once('='))
            .find(|(key, _)| key.trim() == feature)
            .map(|(_, v)| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| feature.into())
    }
    pub fn features(&self, features: &[Feature]) -> Vec<Feature> {
        features
            .iter()
            .filter(|f| {
                (self.show_unused || f.used > 0)
                    && (contains(&f.name, &self.feature)
                        || contains(&self.label(&f.name), &self.feature))
            })
            .filter_map(|f| {
                let users: Vec<_> = f
                    .users
                    .iter()
                    .filter(|u| {
                        contains(&u.user, &self.user) && contains(&u.machine, &self.machine)
                    })
                    .cloned()
                    .collect();
                if (!self.user.trim().is_empty() || !self.machine.trim().is_empty())
                    && users.is_empty()
                {
                    return None;
                }
                Some(Feature {
                    name: f.name.clone(),
                    total: f.total,
                    used: f.used,
                    users,
                })
            })
            .collect()
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}
#[derive(Clone, Debug)]
pub enum Data {
    System(System),
    Licences(Vec<Feature>),
    Text(String),
    Value(String),
    Table(Table),
}
pub fn parse_custom(text: &str, format: &Format) -> Result<Data, String> {
    text_limit(text)?;
    match format {
        Format::Text => {
            if text.lines().any(|line| line.chars().count() > 2048) {
                return Err(
                    "Text lines must be at most 2048 characters; return a shorter summary.".into(),
                );
            }
            let mut preview: String = text.chars().take(32768).collect();
            if text.chars().count() > 32768 {
                preview.push_str("\n[Output truncated to 32,768 characters]");
            }
            Ok(Data::Text(preview))
        }
        Format::Value => {
            let value = text.trim();
            if value.chars().count() > 120 || value.contains(['\n', '\r']) {
                return Err("A value must be one line, up to 120 characters.".into());
            }
            Ok(Data::Value(value.into()))
        }
        Format::Table => {
            let table: Table = serde_json::from_str(text)
                .map_err(|_| "Expected JSON with columns and rows of strings.")?;
            if table.columns.is_empty()
                || table.columns.len() > 12
                || table.rows.len() > 256
                || table.columns.iter().any(|v| v.len() > 128)
                || table
                    .rows
                    .iter()
                    .any(|r| r.len() != table.columns.len() || r.iter().any(|c| c.len() > 2048))
            {
                return Err(
                    "Table must have 1–12 columns, at most 256 rows and bounded cells.".into(),
                );
            }
            Ok(Data::Table(table))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn licence_filters_preserve_counts_and_match_users_machines_and_aliases() {
        let raw="Users of feature_a:  (Total of 10 licenses issued;  Total of 2 licenses in use)\n  user1 host-a :1 (v1.0) (lic/5280 12), start Thu 01/02 09:00\n  user2 other :2 (v1.0) (lic/5280 13), start Thu 01/02 09:01\nUsers of unused:  (Total of 5 licenses issued;  Total of 0 licenses in use)\n";
        let report = parse_licences(raw).unwrap();
        let seats = parse_licences("Users of feature_multi: (Total of 20 licenses issued; Total of 4 licenses in use)\n  AB host-a localhost:10.0 FEATURE_MULTI (v2.0) (license-server.example/5280 1234), start Thu 01/02 09:30, 4 licenses\n").unwrap();
        assert_eq!(seats[0].users.len(), 1);
        assert_eq!(seats[0].users[0].count, 4);
        assert_eq!(seats[0].users[0].since, "Thu 01/02 09:30");
        assert_eq!(seats[0].users[0].display, "localhost:10.0");
        let mut filters = Filters::default();
        assert_eq!(filters.features(&report).len(), 1);
        filters.aliases = "feature_a=Layout editor".into();
        filters.feature = "layout".into();
        filters.user = "user1".into();
        filters.machine = "HOST-A".into();
        let visible = filters.features(&report);
        assert_eq!(visible[0].used, 2); // server total is not a filtered user count
        assert_eq!(visible[0].users.len(), 1);
        assert_eq!(visible[0].users[0].user, "user1");
        filters.machine = "missing".into();
        assert!(filters.features(&report).is_empty());
        assert!(parse_licences("Cannot connect to licence server").is_err());
    }
    #[test]
    fn monitor_commands_quote_parameters_and_validate_custom_output_bounds() {
        let mut m = Monitor::new("profile".into(), true);
        m.kind = Kind::FlexNet {
            executable: "/space dir/lmutil".into(),
            server: "5280@license-server.example'; touch /tmp/no".into(),
        };
        assert!(m
            .command()
            .unwrap()
            .contains("'5280@license-server.example'\\''; touch /tmp/no'"));
        m.kind = Kind::FlexNet {
            executable: "/opt/vendor/tools/bin/lmstat".into(),
            server: "5280@lic".into(),
        };
        assert!(m
            .command()
            .unwrap()
            .contains("'/opt/vendor/tools/bin/lmstat' -c '5280@lic' -a -t 5"));
        m.interval = 1;
        assert!(m.validate().is_err());
        assert!(parse_custom(
            "{\"columns\":[\"Tool\"],\"rows\":[[\"A\"]]}",
            &Format::Table
        )
        .is_ok());
        assert!(parse_custom("{\"columns\":[\"Tool\"],\"rows\":[[]]}", &Format::Table).is_err());
        assert!(parse_custom("a\nb", &Format::Value).is_err());
        assert!(parse_custom(&"a".repeat(MAX_OUTPUT + 1), &Format::Text).is_err());
    }
    #[test]
    fn system_parser_uses_deltas_and_preserves_mount_names_and_login_sessions() {
        let fast=parse_system("AETHER_MONITOR_V1\nHOST\thost-a\nCPU\t1000\t200\nMEM\t100\t40\t20\t15\nUP\t100\nNET\teth0\t1200\t300\n",false).unwrap();
        assert_eq!(fast.memory, Some((100, 40, 20, 15)));
        assert_eq!(cpu_percent((1000, 200), (1100, 225)), Some(75.0));
        assert_eq!(cpu_percent((1000, 200), (1, 0)), None);
        let slow=parse_system("AETHER_MONITOR_V1\nUSERS_BEGIN\nuser1 pts/0 2025-01-02 09:00 (host)\nuser1 pts/1 2025-01-02 09:01\nUSERS_END\nDISKS_BEGIN\nFilesystem 1024-blocks Used Available Capacity Mounted on\n/dev/sda 100 50 45 53% /space dir\nDISKS_END\n",true).unwrap();
        let homes = parse_system("AETHER_MONITOR_V1\nHOME_DIR\tAB\t/home/AB\nHOME_DIR\tCD\t/home/CD\nHOME_SIZE\t128\t/home/AB\nERR\thomes\tpartial\n", true).unwrap();
        assert_eq!(homes.homes[0].kib, Some(128));
        assert_eq!(homes.homes[1].kib, None);
        assert!(homes.homes_error.is_some());
        assert_eq!(slow.users.len(), 2);
        assert_eq!(slow.disks[0].0, "/space dir");
        assert!(parse_system("Linux error", false).is_err());
    }
}
