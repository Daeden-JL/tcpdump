//! The TOML configuration: schema, loading and validation.

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: String,
    #[serde(default)]
    pub site: Site,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub estimate: EstimateConfig,
    pub targets: Vec<Target>,
    #[serde(default)]
    pub flags: Vec<Flag>,
    #[serde(default)]
    pub filter: FilterConfig,
    #[serde(default)]
    pub examples: Vec<Example>,
}

/// Inputs for the capture size estimate.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EstimateConfig {
    /// Link speeds offered for the worst-case estimate of timed captures.
    #[serde(default = "d_link_speeds")]
    pub link_speeds_gbit: Vec<u64>,
    #[serde(default = "d_link_speed")]
    pub default_link_speed_gbit: u64,
    /// Link speed implied by an interface name: `prefix` followed by a number.
    #[serde(default)]
    pub interface_speeds: Vec<InterfaceSpeed>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceSpeed {
    pub prefix: String,
    pub gbit: u64,
}

impl Default for EstimateConfig {
    fn default() -> Self {
        EstimateConfig { link_speeds_gbit: d_link_speeds(), default_link_speed_gbit: d_link_speed(), interface_speeds: Vec::new() }
    }
}

fn d_link_speeds() -> Vec<u64> {
    vec![1, 10, 25, 40, 100]
}
fn d_link_speed() -> u64 {
    10
}

/// What a flag means for the size estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    SnapLength,
    FileSizeMb,
    FileCount,
    RotateSeconds,
    PacketCount,
}

/// Colours. `light` and `dark` override individual entries of [`PALETTE`].
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    #[serde(default)]
    pub mode: ThemeMode,
    #[serde(default)]
    pub light: BTreeMap<String, String>,
    #[serde(default)]
    pub dark: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    /// Follow the visitor's system setting.
    #[default]
    Auto,
    Light,
    Dark,
}

/// Every colour the page uses: (name in the TOML, light default, dark default).
pub const PALETTE: &[(&str, &str, &str)] = &[
    ("bg", "#f4f6f5", "#0e1513"),
    ("panel", "#ffffff", "#16201c"),
    ("ink", "#17211d", "#e3ece8"),
    ("muted", "#5a6964", "#96a8a1"),
    ("line", "#d5dcd9", "#2b3a35"),
    ("accent", "#0b6e4f", "#4fd1a1"),
    ("accent_ink", "#ffffff", "#06231a"),
    ("soft", "#e4f1ec", "#1d3029"),
    ("code_bg", "#10201a", "#0a100e"),
    ("code_ink", "#d6f5e6", "#c9f2df"),
    ("error", "#a4262c", "#ff8f8f"),
    ("warn", "#8a5a00", "#f0c060"),
];

impl Theme {
    fn block(&self, dark: bool) -> String {
        let overrides = if dark { &self.dark } else { &self.light };
        PALETTE
            .iter()
            .map(|(key, light, dark_default)| {
                let value = overrides.get(*key).map_or(if dark { *dark_default } else { *light }, String::as_str);
                format!("--{}:{value};", key.replace('_', "-"))
            })
            .collect()
    }

    /// The stylesheet served at `/theme.css`.
    pub fn css(&self) -> String {
        let (light, dark) = (self.block(false), self.block(true));
        match self.mode {
            ThemeMode::Light => format!(":root{{color-scheme:light;{light}}}\n"),
            ThemeMode::Dark => format!(":root{{color-scheme:dark;{dark}}}\n"),
            ThemeMode::Auto => format!(
                ":root{{color-scheme:light dark;{light}}}\n@media (prefers-color-scheme:dark){{:root{{{dark}}}}}\n"
            ),
        }
    }

    fn validate(&self) -> Result<(), String> {
        for (mode, colours) in [("light", &self.light), ("dark", &self.dark)] {
            for (key, value) in colours {
                if !PALETTE.iter().any(|p| p.0 == key) {
                    let known: Vec<&str> = PALETTE.iter().map(|p| p.0).collect();
                    return Err(format!("theme.{mode}.{key} is not a colour name; use one of: {}", known.join(", ")));
                }
                let hex = value.strip_prefix('#').unwrap_or("");
                if !matches!(hex.len(), 3 | 4 | 6 | 8) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(format!("theme.{mode}.{key} must be a hex colour such as #0b6e4f, not {value:?}"));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Site {
    /// Public base URL, used for share links. Empty means "whatever the browser is on".
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub tagline: String,
    /// Address the server binds to. Read at startup only.
    #[serde(default)]
    pub listen: String,
    /// Footer link to the source repository.
    #[serde(default)]
    pub repo: Link,
    /// Footer link to the license.
    #[serde(default)]
    pub license: Link,
}

/// An optional footer link; shown only when `enabled` and `url` is set.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "d_command")]
    pub command: String,
    pub output_dir: String,
    #[serde(default = "d_date_subdir")]
    pub date_subdir: String,
    #[serde(default = "d_true")]
    pub dated_default: bool,
    #[serde(default = "d_hostname")]
    pub hostname: String,
    #[serde(default = "d_file_name")]
    pub file_name: String,
    #[serde(default = "d_log_name")]
    pub log_name: String,
    /// `{timestamp}` normally expands to this (evaluated by the shell)...
    #[serde(default = "d_timestamp")]
    pub timestamp: String,
    /// ...but when one of these flags is on, tcpdump runs the file name
    /// through strftime itself, so `strftime_timestamp` is used instead.
    #[serde(default = "d_strftime_flags")]
    pub strftime_flags: Vec<String>,
    #[serde(default = "d_strftime_timestamp")]
    pub strftime_timestamp: String,
    #[serde(default = "d_prepare")]
    pub prepare: String,
    #[serde(default = "d_log_redirect")]
    pub log_redirect: String,
    #[serde(default)]
    pub log_default: bool,
    #[serde(default)]
    pub background_default: bool,
    #[serde(default = "d_stop")]
    pub stop: String,
    /// The `at` command used to schedule a capture.
    #[serde(default = "d_at_command")]
    pub at_command: String,
    /// Log redirect used inside an `at` job, which runs under /bin/sh.
    #[serde(default = "d_log_redirect")]
    pub at_log_redirect: String,
    /// Shown next to the schedule settings.
    #[serde(default)]
    pub at_note: String,
    /// Commands shown for managing queued `at` jobs.
    #[serde(default = "d_at_list")]
    pub at_list: String,
    #[serde(default = "d_at_show")]
    pub at_show: String,
    #[serde(default = "d_at_remove")]
    pub at_remove: String,
    /// Queues an `at` job from a script file: {dir} {script} {command} {at} {spec}.
    #[serde(default = "d_at_script_install")]
    pub at_script_install: String,
    /// Deletes that script: {script}.
    #[serde(default = "d_script_remove")]
    pub script_remove: String,
    /// Folder scripts are written to. Defaults to `output_dir`.
    #[serde(default)]
    pub cron_dir: Option<String>,
    /// Installs a recurring capture: {dir} {script} {expr} {command}.
    #[serde(default = "d_cron_install")]
    pub cron_install: String,
    /// Removes it again: {script}.
    #[serde(default = "d_cron_remove")]
    pub cron_remove: String,
    #[serde(default = "d_cron_list")]
    pub cron_list: String,
    #[serde(default)]
    pub cron_note: String,
    pub default_interface: String,
    #[serde(default)]
    pub interface_hint: String,
    /// Shell expression listing every interface, used in a `for` loop.
    #[serde(default)]
    pub all_interfaces: Option<String>,
    /// A pseudo-interface that captures on everything (Linux `any`).
    #[serde(default)]
    pub any_interface: Option<String>,
    #[serde(default)]
    pub cluster: Option<Cluster>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cluster {
    /// Template wrapping `{command}` to run it on every node.
    pub all_nodes: String,
    /// Template wrapping `{command}` to run it on `{nodes}`.
    pub selected_nodes: String,
    #[serde(default)]
    pub nodes_hint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ValueType {
    Integer,
    String,
    Bool,
    Choice,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Flag {
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    /// Longer background text, shown on demand.
    #[serde(default)]
    pub explanation: String,
    pub value_type: ValueType,
    #[serde(default, deserialize_with = "string_or_number")]
    pub default_value: String,
    /// Whether the flag is switched on in a fresh builder.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub validation: Option<Validation>,
    /// Satisfied when at least one of these flags is on.
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub exclusions: Vec<String>,
    /// Target ids this flag applies to. Empty means all.
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub choices: Vec<String>,
    /// Tells the size estimate what this flag controls.
    #[serde(default)]
    pub role: Option<Role>,
    /// Common values and why you would pick them.
    #[serde(default)]
    pub recommendations: Vec<Recommendation>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Recommendation {
    #[serde(deserialize_with = "string_or_number")]
    pub value: String,
    #[serde(default)]
    pub label: String,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Validation {
    pub min: Option<i64>,
    pub max: Option<i64>,
    /// What Fix uses for a value above `max`, instead of `max` itself
    /// (for snap length, 0: "whole packet" is what a huge value means).
    #[serde(default)]
    pub over_max: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FilterConfig {
    #[serde(default)]
    pub protocols: Vec<String>,
    #[serde(default)]
    pub snippets: Vec<Snippet>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Snippet {
    pub label: String,
    pub expression: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Example {
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Builder state as a URL query string, exactly as it appears in a share link.
    pub query: String,
}

fn d_true() -> bool {
    true
}
fn d_command() -> String {
    "tcpdump".into()
}
fn d_date_subdir() -> String {
    "$(date +%F)".into()
}
fn d_hostname() -> String {
    "$(uname -n)".into()
}
fn d_file_name() -> String {
    "{hostname}.{timestamp}.{iface}.pcap".into()
}
fn d_log_name() -> String {
    "{hostname}.$(date +%F_%H%M%S).{iface}.log".into()
}
fn d_timestamp() -> String {
    "$(date +%F_%H%M%S)".into()
}
fn d_strftime_flags() -> Vec<String> {
    vec!["-G".into()]
}
fn d_strftime_timestamp() -> String {
    "%F_%H%M%S".into()
}
fn d_prepare() -> String {
    "mkdir -p {dir}".into()
}
fn d_log_redirect() -> String {
    ">> {log} 2>&1".into()
}
fn d_at_list() -> String {
    "atq".into()
}
fn d_at_show() -> String {
    "at -c <job number>".into()
}
fn d_at_remove() -> String {
    "atrm <job number>".into()
}
fn d_at_script_install() -> String {
    "mkdir -p {dir}\ncat > {script} <<'EOF'\n#!/bin/sh\n{command}\nEOF\nchmod +x {script}\n{at} -f {script} {spec}".into()
}
fn d_script_remove() -> String {
    "rm -f {script}".into()
}
fn d_cron_install() -> String {
    "mkdir -p {dir}\ncat > {script} <<'EOF'\n#!/bin/sh\n{command}\nEOF\nchmod +x {script}\n(crontab -l 2>/dev/null | grep -v '{script}'; echo '{expr} {script}') | crontab -".into()
}
fn d_cron_remove() -> String {
    "crontab -l | grep -v '{script}' | crontab -; rm -f {script}".into()
}
fn d_cron_list() -> String {
    "crontab -l".into()
}
fn d_at_command() -> String {
    "at".into()
}
fn d_stop() -> String {
    "killall -SIGINT tcpdump".into()
}

fn string_or_number<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum V {
        S(String),
        I(i64),
    }
    Ok(match V::deserialize(d)? {
        V::S(s) => s,
        V::I(i) => i.to_string(),
    })
}

impl Flag {
    /// URL parameter suffix: the flag name without its leading dashes.
    pub fn key(&self) -> &str {
        self.name.trim_start_matches('-')
    }

    pub fn applies_to(&self, target: &str) -> bool {
        self.targets.is_empty() || self.targets.iter().any(|t| t == target)
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Config::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        let cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), String> {
        if self.targets.is_empty() {
            return Err("at least one [[targets]] entry is required".into());
        }
        for (name, link) in [("site.repo", &self.site.repo), ("site.license", &self.site.license)] {
            let web = link.url.starts_with("https://") || link.url.starts_with("http://");
            if link.enabled && !web {
                return Err(format!("{name}.url must start with https:// or http:// while the link is enabled"));
            }
        }
        self.theme.validate()?;
        if !self.estimate.link_speeds_gbit.contains(&self.estimate.default_link_speed_gbit) {
            return Err("estimate.default_link_speed_gbit must be one of estimate.link_speeds_gbit".into());
        }
        if self.estimate.link_speeds_gbit.iter().any(|s| *s == 0 || *s > 1000) {
            return Err("estimate.link_speeds_gbit entries must be between 1 and 1000".into());
        }
        for s in &self.estimate.interface_speeds {
            if s.prefix.is_empty() || !s.prefix.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || s.gbit == 0 || s.gbit > 1000 {
                return Err(format!("estimate.interface_speeds entry {:?} needs a plain name prefix and a speed from 1 to 1000", s.prefix));
            }
        }
        let mut roles = Vec::new();
        for f in self.flags.iter().filter(|f| f.role.is_some()) {
            if f.value_type != ValueType::Integer {
                return Err(format!("flag {} has a role, so its value_type must be Integer", f.name));
            }
            if roles.contains(&f.role) {
                return Err(format!("flag {} repeats a role another flag already has", f.name));
            }
            roles.push(f.role);
        }
        let mut ids = HashSet::new();
        for t in &self.targets {
            if t.id.is_empty() || !t.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                return Err(format!("target id {:?} must be letters, digits, '-' or '_'", t.id));
            }
            if !ids.insert(&t.id) {
                return Err(format!("duplicate target id {:?}", t.id));
            }
        }
        let mut names = HashSet::new();
        for f in &self.flags {
            let key = f.key();
            if !f.name.starts_with('-') || key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(format!("flag name {:?} must look like -x or --long-name", f.name));
            }
            if f.name == "-i" || f.name == "-w" {
                return Err(format!("{} is generated from the target settings and cannot be listed under [[flags]]", f.name));
            }
            if !names.insert(key) {
                return Err(format!("duplicate flag {:?}", f.name));
            }
            if let Some(v) = &f.validation {
                let outside = |n: i64| v.min.is_some_and(|m| n < m) || v.max.is_some_and(|m| n > m);
                if v.over_max.is_some_and(outside) {
                    return Err(format!("flag {}: validation.over_max must itself be between min and max", f.name));
                }
            }
            if f.value_type == ValueType::Choice && f.choices.is_empty() {
                return Err(format!("flag {} has value_type Choice but no choices", f.name));
            }
            for t in &f.targets {
                if !ids.contains(t) {
                    return Err(format!("flag {} lists unknown target {t:?}", f.name));
                }
            }
        }
        for f in &self.flags {
            for other in f.dependencies.iter().chain(&f.exclusions) {
                if !self.flags.iter().any(|x| &x.name == other) {
                    return Err(format!("flag {} refers to unknown flag {other:?}", f.name));
                }
            }
        }
        for ex in &self.examples {
            let pairs = crate::builder::parse_query(&ex.query);
            let out = crate::builder::build(self, &pairs);
            if let Some(i) = out.issues.iter().find(|i| i.level == "error") {
                return Err(format!("example {:?} does not build: {}", ex.title, i.message));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIPPED: &str = include_str!("../tcpdump.toml");

    #[test]
    fn footer_links_can_be_switched_off_or_omitted() {
        let cfg = Config::parse(SHIPPED).unwrap();
        assert!(cfg.site.repo.enabled && cfg.site.license.enabled);

        let off = SHIPPED.replacen("enabled = true\nurl = \"https://github.com/Daeden-JL/tcpdump\"", "enabled = false\nurl = \"\"", 1);
        let cfg = Config::parse(&off).unwrap();
        assert!(!cfg.site.repo.enabled && cfg.site.license.enabled);

        let minimal = "version = \"1\"\n[[targets]]\nid = \"x\"\nname = \"X\"\noutput_dir = \"/tmp\"\ndefault_interface = \"eth0\"\n";
        assert!(!Config::parse(minimal).unwrap().site.repo.enabled);
    }

    #[test]
    fn enabled_footer_links_need_a_web_address() {
        let bad = SHIPPED.replacen("url = \"https://github.com/Daeden-JL/tcpdump\"", "url = \"javascript:alert(1)\"", 1);
        assert!(Config::parse(&bad).unwrap_err().contains("site.repo.url"));
    }

    #[test]
    fn theme_modes_and_overrides() {
        let cfg = Config::parse(SHIPPED).unwrap();
        let css = cfg.theme.css();
        assert!(css.contains("color-scheme:light dark") && css.contains("prefers-color-scheme:dark"));
        assert!(css.contains("--accent-ink:#ffffff;") && css.contains("--bg:#0e1513;"));

        let forced = SHIPPED.replacen("mode = \"auto\"", "mode = \"dark\"", 1).replacen("accent = \"#4fd1a1\"", "accent = \"#ff00aa\"", 1);
        let css = Config::parse(&forced).unwrap().theme.css();
        assert!(css.starts_with(":root{color-scheme:dark;") && !css.contains("@media"));
        assert!(css.contains("--accent:#ff00aa;") && !css.contains("#f4f6f5"));

        let minimal = "version = \"1\"\n[theme.light]\naccent = \"#123\"\n[[targets]]\nid = \"x\"\nname = \"X\"\noutput_dir = \"/tmp\"\ndefault_interface = \"eth0\"\n";
        let css = Config::parse(minimal).unwrap().theme.css();
        assert!(css.contains("--accent:#123;") && css.contains("--panel:#ffffff;"));
    }

    #[test]
    fn theme_rejects_unknown_names_and_unsafe_values() {
        let head = "version = \"1\"\n[[targets]]\nid = \"x\"\nname = \"X\"\noutput_dir = \"/tmp\"\ndefault_interface = \"eth0\"\n";
        for bad in ["[theme.light]\nbackground = \"#fff\"", "[theme.dark]\nbg = \"red;} body{display:none\"", "[theme]\nmode = \"neon\""] {
            assert!(Config::parse(&format!("{head}{bad}\n")).is_err(), "{bad}");
        }
    }
}
