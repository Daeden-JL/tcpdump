//! The TOML configuration: schema, loading and validation.

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: String,
    #[serde(default)]
    pub site: Site,
    pub targets: Vec<Target>,
    #[serde(default)]
    pub flags: Vec<Flag>,
    #[serde(default)]
    pub filter: FilterConfig,
    #[serde(default)]
    pub examples: Vec<Example>,
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
