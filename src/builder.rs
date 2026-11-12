//! Turns a URL query string into builder state and a shell command.
//!
//! The generated command is meant to be pasted into a root shell, and the
//! query string can come from a link someone else sent. Every user-supplied
//! value is therefore checked against a conservative character set, and no
//! command is produced at all while any error is outstanding.

use crate::config::{Config, Flag, Role, Target, ValueType};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Serialize)]
pub struct Issue {
    pub level: &'static str,
    pub field: String,
    pub message: String,
}

/// The resolved form state, sent back so the page can fill itself in.
#[derive(Debug, Clone, Serialize)]
pub struct State {
    pub target: String,
    pub scope: String,
    pub nodes: String,
    pub iface_mode: String,
    pub ifaces: Vec<String>,
    pub write: bool,
    pub subdir: String,
    pub dated: bool,
    pub log: bool,
    pub background: bool,
    /// Enabled flags: key (name without dashes) to value.
    pub flags: BTreeMap<String, String>,
    pub hosts: Vec<String>,
    pub ports: Vec<String>,
    pub nets: Vec<String>,
    pub proto: String,
    pub raw: String,
    /// Scheduling with `at`. Start mode is "now", "time", "date" or "delay";
    /// stop mode is "none" or one of the last three. The value is HH:MM,
    /// YYYY-MM-DDTHH:MM or a number, and the unit applies to a delay.
    pub at_start: String,
    pub at_start_value: String,
    pub at_start_unit: String,
    pub at_stop: String,
    pub at_stop_value: String,
    pub at_stop_unit: String,
    /// Queue the `at` job from a script file instead of inline.
    pub at_script: bool,
    /// Estimate inputs: counts the command cannot tell us, link speed in
    /// Gbit/s, and free space in GB. They never change the command.
    pub est_nodes: String,
    pub est_ifaces: String,
    pub link_speed: String,
    pub free_space: String,
    /// Recurring capture (start mode "cron"): kind is "daily", "weekdays",
    /// "hours" or "custom".
    pub cron_kind: String,
    pub cron_time: String,
    pub cron_days: String,
    pub cron_hours: String,
    pub cron_expr: String,
    pub cron_name: String,
    /// Values typed in for placeholders, keyed by the placeholder (`<interface>`).
    pub fills: BTreeMap<String, String>,
}

/// A placeholder in the settings and what has been filled in for it.
#[derive(Debug, Serialize)]
pub struct Placeholder {
    pub name: String,
    pub hint: &'static str,
    pub value: String,
}

/// A command for looking after a scheduled capture.
#[derive(Debug, Serialize)]
pub struct Manage {
    pub label: &'static str,
    pub command: String,
}

/// The capture size estimate, already worded for display.
#[derive(Debug, Serialize)]
pub struct Estimate {
    /// "none" (nothing written), "exact", "worst" or "unbounded".
    pub kind: &'static str,
    pub headline: String,
    pub lines: Vec<String>,
    pub need_nodes: bool,
    pub need_ifaces: bool,
    pub need_link: bool,
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub command: String,
    pub stop_command: Option<String>,
    pub issues: Vec<Issue>,
    pub estimate: Option<Estimate>,
    pub placeholders: Vec<Placeholder>,
    /// The command explained in a few plain sentences.
    pub summary: Vec<String>,
    /// One line saying when a scheduled capture runs.
    pub schedule_summary: Option<String>,
    pub manage: Vec<Manage>,
    pub state: State,
}

pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .trim_start_matches('?')
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = |c: u8| (c as char).to_digit(16);
                match (hex(b[i + 1]), hex(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Single-pass `{name}` substitution; unknown names are left untouched and
/// substituted values are never rescanned.
fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let hit = after.find('}').and_then(|end| {
            let name = &after[..end];
            vars.iter().find(|(k, _)| *k == name).map(|(_, v)| (*v, end))
        });
        match hit {
            Some((value, end)) => {
                out.push_str(value);
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A value left for the person running the command to fill in: `<interface>`.
pub fn is_placeholder(s: &str) -> bool {
    let inner = s.strip_prefix('<').and_then(|r| r.strip_suffix('>')).unwrap_or("");
    !inner.trim().is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric() || " _-".contains(c))
}

/// Split on whitespace (and commas, if asked), keeping `<a placeholder>` whole.
pub fn split_atoms(s: &str, commas: bool) -> Vec<String> {
    let mut out = Vec::new();
    let (mut cur, mut inside) = (String::new(), false);
    for c in s.chars() {
        inside |= c == '<';
        if !inside && (c.is_whitespace() || (commas && c == ',')) {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
        inside &= c != '>';
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn split_list(s: &str) -> Vec<String> {
    split_atoms(s, true)
}

/// Every placeholder that appears in `s`, in order.
fn placeholders_in(s: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find('<') {
        match rest[start..].find('>') {
            Some(len) => {
                let token = &rest[start..=start + len];
                if is_placeholder(token) {
                    found.push(token.to_string());
                }
                rest = &rest[start + len + 1..];
            }
            None => break,
        }
    }
    found
}

fn replace_placeholders(s: &str, fills: &BTreeMap<String, String>) -> String {
    placeholders_in(s).iter().fold(s.to_string(), |text, token| match fills.get(token) {
        Some(value) => text.replace(token.as_str(), value),
        None => text,
    })
}

/// Where a placeholder sits, which decides what may be filled into it.
#[derive(Clone, Copy, PartialEq)]
enum Use {
    Iface,
    Nodes,
    Subdir,
    Host,
    Net,
    Port,
    Raw,
}

impl Use {
    fn label(self) -> &'static str {
        match self {
            Use::Iface => "Interface name, or several separated by commas",
            Use::Nodes => "Node numbers, e.g. 1,2 or 1-4",
            Use::Subdir => "Subfolder name",
            Use::Host => "Host name or IP address, or several separated by commas",
            Use::Net => "Network, e.g. 172.26.0.0/16",
            Use::Port => "Port or port range, e.g. 445 or 8000-8100",
            Use::Raw => "Filter text",
        }
    }
}

/// Placeholders in the settings that are actually used by the command.
fn placeholder_uses(st: &State) -> Vec<(String, Use)> {
    let mut uses = Vec::new();
    let mut list = |items: &[String], kind: Use| uses.extend(items.iter().filter(|i| is_placeholder(i)).map(|i| (i.clone(), kind)));
    if st.iface_mode == "named" {
        list(&st.ifaces, Use::Iface);
    }
    list(&st.hosts, Use::Host);
    list(&st.nets, Use::Net);
    list(&st.ports, Use::Port);
    if st.scope == "nodes" {
        uses.extend(placeholders_in(&st.nodes).into_iter().map(|p| (p, Use::Nodes)));
    }
    if st.write {
        uses.extend(placeholders_in(&st.subdir).into_iter().map(|p| (p, Use::Subdir)));
    }
    uses.extend(placeholders_in(&st.raw).into_iter().map(|p| (p, Use::Raw)));
    uses
}

/// Why `value` cannot go where the placeholder is, if it cannot.
fn fill_problem(value: &str, kind: Use) -> Option<&'static str> {
    let each = |ok: fn(&str) -> bool| {
        let items = split_list(value);
        !items.is_empty() && items.iter().all(|i| ok(i) && !is_placeholder(i))
    };
    let fine = match kind {
        Use::Iface => each(valid_iface),
        Use::Host => each(valid_host),
        Use::Net => each(valid_net),
        Use::Port => each(valid_port),
        Use::Nodes => valid_nodes(value) && !is_placeholder(value),
        Use::Subdir => valid_subdir(value) && !value.contains('<'),
        Use::Raw => {
            let mut probe = Ctx { issues: Vec::new() };
            check_raw_filter(value, &mut probe);
            probe.issues.is_empty() && !value.contains('<')
        }
    };
    (!fine).then_some(match kind {
        Use::Iface => "Enter an interface name such as lagg0, or several separated by commas.",
        Use::Host => "Enter a host name or IP address, or several separated by commas.",
        Use::Net => "Enter a network such as 172.26.0.0/16.",
        Use::Port => "Enter a port or a range such as 445 or 8000-8100.",
        Use::Nodes => "Enter node numbers such as 1,2 or 1-4.",
        Use::Subdir => "Use only letters, digits and _ . - / here.",
        Use::Raw => "That text cannot be used in a filter.",
    })
}

/// The settings with every usable fill-in value put in place of its placeholder.
fn with_fills(st: &State, fills: &BTreeMap<String, String>) -> State {
    let expand = |items: &[String]| -> Vec<String> {
        items.iter().flat_map(|i| fills.get(i).map_or_else(|| vec![i.clone()], |v| split_list(v))).collect()
    };
    State {
        ifaces: expand(&st.ifaces),
        hosts: expand(&st.hosts),
        nets: expand(&st.nets),
        ports: expand(&st.ports),
        nodes: replace_placeholders(&st.nodes, fills),
        subdir: replace_placeholders(&st.subdir, fills),
        raw: replace_placeholders(&st.raw, fills),
        ..st.clone()
    }
}

fn all_chars(s: &str, extra: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

fn valid_port(p: &str) -> bool {
    if is_placeholder(p) {
        return true;
    }
    let ok = |x: &str| x.parse::<u16>().is_ok();
    match p.split_once('-') {
        Some((a, b)) => ok(a) && ok(b),
        None => ok(p),
    }
}

fn valid_iface(s: &str) -> bool {
    is_placeholder(s) || all_chars(s, "_.:-")
}

fn valid_host(s: &str) -> bool {
    is_placeholder(s) || all_chars(s, ".:-")
}

fn valid_net(s: &str) -> bool {
    is_placeholder(s) || (all_chars(s, ".:/") && s.starts_with(|c: char| c.is_ascii_hexdigit()))
}

pub fn valid_subdir(s: &str) -> bool {
    !s.is_empty() && s.split('/').all(|p| is_placeholder(p) || (all_chars(p, "_.-") && p != ".."))
}

fn valid_nodes(s: &str) -> bool {
    is_placeholder(s) || (all_chars(s, ",-") && s.starts_with(|c: char| c.is_ascii_digit()))
}

const CRON_DEFAULTS: [(&str, &str); 5] =
    [("time", "01:00"), ("days", "1,2,3,4,5"), ("hours", "6"), ("expr", "0 1 * * *"), ("name", "capture")];

fn valid_cron_name(s: &str) -> bool {
    all_chars(s, "_-") && s.len() <= 32
}

/// The five-field cron expression for the repeat settings, or the field at
/// fault and what is wrong with it.
fn cron_expr(st: &State) -> Result<String, (&'static str, &'static str)> {
    let clock = || -> Result<(u32, u32), (&'static str, &'static str)> {
        match st.cron_time.split_once(':') {
            Some((h, m)) if two_digits(h, 23) && two_digits(m, 59) => Ok((h.parse().unwrap_or(0), m.parse().unwrap_or(0))),
            _ => Err(("cron_time", "Enter the time as HH:MM, for example 01:00.")),
        }
    };
    match st.cron_kind.as_str() {
        "daily" => clock().map(|(h, m)| format!("{m} {h} * * *")),
        "weekdays" => {
            let mut days: Vec<u32> = Vec::new();
            for d in st.cron_days.split(',').filter(|d| !d.is_empty()) {
                match d.parse::<u32>() {
                    Ok(n) if n <= 6 => days.push(n),
                    _ => return Err(("cron_days", "Pick at least one day of the week.")),
                }
            }
            days.sort_unstable();
            days.dedup();
            if days.is_empty() {
                return Err(("cron_days", "Pick at least one day of the week."));
            }
            let list: Vec<String> = days.iter().map(u32::to_string).collect();
            clock().map(|(h, m)| format!("{m} {h} * * {}", list.join(",")))
        }
        "hours" => match st.cron_hours.parse::<u32>() {
            Ok(n) if (1..=23).contains(&n) => Ok(format!("0 */{n} * * *")),
            _ => Err(("cron_hours", "Enter a whole number of hours from 1 to 23.")),
        },
        _ => {
            let fields: Vec<&str> = st.cron_expr.split_whitespace().collect();
            let ok = fields.len() == 5 && fields.iter().all(|f| f.chars().all(|c| c.is_ascii_digit() || "*,/-".contains(c)));
            if ok { Ok(fields.join(" ")) } else { Err(("cron_expr", "Enter five fields made of digits and * , / - (minute hour day month weekday), for example 0 1 * * 1-5.")) }
        }
    }
}

/// Say a cron expression in words, for the common shapes.
fn describe_cron(expr: &str) -> String {
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    let f: Vec<&str> = expr.split(' ').collect();
    let num = |s: &str| s.parse::<u32>().ok();
    match f[..] {
        [m, h, "*", "*", dow] if num(m).is_some() && num(h).is_some() => {
            let at = format!("{:02}:{:02}", num(h).unwrap_or(0), num(m).unwrap_or(0));
            if dow == "*" {
                return format!("Runs every day at {at}.");
            }
            let names: Option<Vec<&str>> = dow.split(',').map(|d| num(d).and_then(|n| DAYS.get(n as usize).copied())).collect();
            match names {
                Some(names) => format!("Runs at {at} on {}.", names.join(", ")),
                None => format!("Runs on the schedule \"{expr}\"."),
            }
        }
        ["0", h, "*", "*", "*"] if h.strip_prefix("*/").and_then(num).is_some() => {
            format!("Runs every {} hours, on the hour.", &h[2..])
        }
        _ => format!("Runs on the schedule \"{expr}\" (minute hour day month weekday)."),
    }
}

/// Wrap in double quotes when the shell would otherwise interpret something.
fn shell_word(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || " ./:_-".contains(c)) {
        s.to_string()
    } else {
        format!("\"{s}\"")
    }
}

struct Ctx {
    issues: Vec<Issue>,
}

impl Ctx {
    fn error(&mut self, field: &str, message: impl Into<String>) {
        self.issues.push(Issue { level: "error", field: field.into(), message: message.into() });
    }
    fn warn(&mut self, field: &str, message: impl Into<String>) {
        self.issues.push(Issue { level: "warning", field: field.into(), message: message.into() });
    }
}

fn check_raw_filter(raw: &str, cx: &mut Ctx) {
    const ALLOWED: &str = " .:/-_()[]&|!=<>+*,";
    if let Some(bad) = raw.chars().find(|c| !c.is_ascii_alphanumeric() && !ALLOWED.contains(*c)) {
        cx.error("raw", format!("The filter expression cannot contain {bad:?}."));
    }
    let b = raw.as_bytes();
    if b.iter().enumerate().any(|(i, c)| *c == b'!' && b.get(i + 1) != Some(&b'=')) {
        cx.error("raw", "Write \"not\" instead of \"!\" in the filter; shells treat \"!\" specially.");
    }
}

fn build_filter(cfg: &Config, st: &State, cx: &mut Ctx) -> String {
    let mut groups: Vec<Vec<String>> = Vec::new();

    let mut addrs = Vec::new();
    for h in &st.hosts {
        if !valid_host(h) {
            cx.error("hosts", format!("{h:?} is not a valid host name or IP address."));
        }
        addrs.push(format!("host {h}"));
    }
    for n in &st.nets {
        if !valid_net(n) {
            cx.error("nets", format!("{n:?} is not a valid network, e.g. 172.26.0.0/16."));
        }
        addrs.push(format!("net {n}"));
    }
    if !addrs.is_empty() {
        groups.push(addrs);
    }

    let mut ports = Vec::new();
    for p in &st.ports {
        if !valid_port(p) {
            cx.error("ports", format!("{p:?} is not a valid port or port range (e.g. 445 or 8000-8100)."));
        }
        ports.push(if p.contains('-') { format!("portrange {p}") } else { format!("port {p}") });
    }
    if !ports.is_empty() {
        groups.push(ports);
    }

    if !st.proto.is_empty() {
        if !cfg.filter.protocols.iter().any(|p| p == &st.proto) {
            cx.error("proto", format!("Unknown protocol {:?}.", st.proto));
        }
        groups.push(vec![st.proto.clone()]);
    }

    let raw = st.raw.trim();
    let many = groups.len() + usize::from(!raw.is_empty()) > 1;
    let mut parts: Vec<String> = groups
        .into_iter()
        .map(|g| {
            let joined = g.join(" or ");
            if many && g.len() > 1 { format!("( {joined} )") } else { joined }
        })
        .collect();
    if !raw.is_empty() {
        check_raw_filter(raw, cx);
        parts.push(if many { format!("( {raw} )") } else { raw.to_string() });
    }
    shell_word(&parts.join(" and "))
}

fn resolve_flags(cfg: &Config, target: &Target, q: &HashMap<&str, &str>, explicit: bool) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for f in cfg.flags.iter().filter(|f| f.applies_to(&target.id)) {
        let given = q.get(format!("f.{}", f.key()).as_str());
        let value = match given {
            Some(v) if explicit => Some(v.to_string()),
            _ if !explicit && (f.enabled || f.required) => Some(f.default_value.clone()),
            _ if f.required => Some(f.default_value.clone()),
            _ => None,
        };
        if let Some(v) = value {
            out.insert(f.key().to_string(), if f.value_type == ValueType::Bool { String::new() } else { v });
        }
    }
    out
}

fn check_flag(f: &Flag, value: &str, enabled: &dyn Fn(&str) -> bool, cx: &mut Ctx) {
    let field = format!("f.{}", f.key());
    match f.value_type {
        ValueType::Bool => {}
        ValueType::Integer => match value.parse::<i64>() {
            Err(_) => cx.error(&field, format!("{} needs a whole number.", f.name)),
            Ok(n) => {
                if let Some(v) = &f.validation {
                    if v.min.is_some_and(|m| n < m) || v.max.is_some_and(|m| n > m) {
                        let lo = v.min.map_or("-inf".into(), |m| m.to_string());
                        let hi = v.max.map_or("inf".into(), |m| m.to_string());
                        cx.error(&field, format!("{} must be between {lo} and {hi}.", f.name));
                    }
                }
            }
        },
        ValueType::Choice => {
            if !f.choices.iter().any(|c| c == value) {
                cx.error(&field, format!("{} must be one of: {}.", f.name, f.choices.join(", ")));
            }
        }
        ValueType::String => {
            if !all_chars(value, "_./:=,+@%-") {
                cx.error(&field, format!("{} needs a value made of letters, digits and _ . / : = , + @ % -", f.name));
            }
        }
    }
    if !f.dependencies.is_empty() && !f.dependencies.iter().any(|d| enabled(d)) {
        cx.error(&field, format!("{} only works together with {}.", f.name, f.dependencies.join(" or ")));
    }
    for x in f.exclusions.iter().filter(|x| enabled(x)) {
        cx.error(&field, format!("{} cannot be combined with {x}.", f.name));
    }
}

/// Whether `value` is acceptable for the flag on its own (ignoring other flags).
fn value_ok(f: &Flag, value: &str) -> bool {
    match f.value_type {
        ValueType::Bool => true,
        ValueType::Integer => value.parse::<i64>().is_ok_and(|n| {
            let v = f.validation.as_ref();
            !v.and_then(|v| v.min).is_some_and(|m| n < m) && !v.and_then(|v| v.max).is_some_and(|m| n > m)
        }),
        ValueType::Choice => f.choices.iter().any(|c| c == value),
        ValueType::String => all_chars(value, "_./:=,+@%-"),
    }
}

/// The closest acceptable value: the nearer limit for a number out of range
/// (or the flag's `over_max` when it is too large), otherwise the default. `None` when there is nothing usable.
fn nearest_value(f: &Flag, value: &str) -> Option<String> {
    if let (ValueType::Integer, Ok(n)) = (f.value_type, value.parse::<i64>()) {
        let v = f.validation.as_ref();
        if let Some(min) = v.and_then(|v| v.min).filter(|m| n < *m) {
            return Some(min.to_string());
        }
        if let Some(max) = v.and_then(|v| v.max).filter(|m| n > *m) {
            return Some(v.and_then(|v| v.over_max).unwrap_or(max).to_string());
        }
    }
    if value_ok(f, &f.default_value) {
        return Some(f.default_value.clone());
    }
    f.choices.first().cloned().filter(|_| f.value_type == ValueType::Choice)
}

pub struct Fixed {
    /// What was changed, in plain words, for the confirmation prompt.
    pub changes: Vec<String>,
    pub output: Output,
}

/// Repair every setting that `build` would reject and report what changed.
pub fn fix(cfg: &Config, pairs: &[(String, String)]) -> Fixed {
    let mut cx = Ctx { issues: Vec::new() };
    let mut st = resolve(cfg, pairs, &mut cx);
    let target = target_of(cfg, &st);
    let mut changes: Vec<String> = Vec::new();
    if !cx.issues.is_empty() {
        changes.push(format!("System set to {}.", target.name));
    }
    let shown = |v: &str| if v.is_empty() { "(empty)".to_string() } else { v.to_string() };

    for f in &cfg.flags {
        let Some(value) = st.flags.get(f.key()).cloned() else { continue };
        if value_ok(f, &value) {
            continue;
        }
        match nearest_value(f, &value) {
            Some(new) => {
                changes.push(format!("{} changed from {} to {new}.", f.name, shown(&value)));
                st.flags.insert(f.key().to_string(), new);
            }
            None if !f.required => {
                changes.push(format!("{} switched off: {} is not a usable value.", f.name, shown(&value)));
                st.flags.remove(f.key());
            }
            None => {}
        }
    }
    // Clashes first, then anything left without the flag it depends on.
    let on = |st: &State, name: &str| st.flags.contains_key(name.trim_start_matches('-'));
    for f in cfg.flags.iter().filter(|f| !f.required) {
        if on(&st, &f.name) {
            if let Some(other) = f.exclusions.iter().find(|x| on(&st, x)) {
                changes.push(format!("{} switched off: it cannot be combined with {other}.", f.name));
                st.flags.remove(f.key());
            }
        }
    }
    while let Some(f) = cfg.flags.iter().find(|f| {
        !f.required && on(&st, &f.name) && !f.dependencies.is_empty() && !f.dependencies.iter().any(|d| on(&st, d))
    }) {
        changes.push(format!("{} switched off: it needs {}.", f.name, f.dependencies.join(" or ")));
        st.flags.remove(f.key());
    }

    fn drop_invalid(list: &mut Vec<String>, what: &str, ok: fn(&str) -> bool, changes: &mut Vec<String>) {
        list.retain(|item| {
            let keep = ok(item);
            if !keep {
                changes.push(format!("{what} {item:?} removed."));
            }
            keep
        });
    }
    if st.iface_mode == "named" {
        drop_invalid(&mut st.ifaces, "Interface", valid_iface, &mut changes);
        if st.ifaces.is_empty() {
            st.ifaces.push(target.default_interface.clone());
            changes.push(format!("Interface set to {}.", target.default_interface));
        }
    }
    drop_invalid(&mut st.hosts, "Host", valid_host, &mut changes);
    drop_invalid(&mut st.nets, "Network", valid_net, &mut changes);
    drop_invalid(&mut st.ports, "Port", valid_port, &mut changes);
    if st.scope == "nodes" && !valid_nodes(&st.nodes) {
        changes.push(format!("Node list {} is not valid; changed to \"This node\".", shown(&st.nodes)));
        st.scope = "node".into();
        st.nodes.clear();
    }
    if !st.subdir.is_empty() && !valid_subdir(&st.subdir) {
        changes.push(format!("Subfolder {:?} removed.", st.subdir));
        st.subdir.clear();
    }
    if !st.proto.is_empty() && !cfg.filter.protocols.iter().any(|p| p == &st.proto) {
        changes.push(format!("Protocol {:?} removed.", st.proto));
        st.proto.clear();
    }
    // Fill-in values that cannot go where their placeholder is.
    for (name, kind) in placeholder_uses(&st) {
        if st.fills.get(&name).is_some_and(|v| fill_problem(v, kind).is_some()) {
            changes.push(format!("The value entered for {name} ({:?}) removed.", st.fills[&name]));
            st.fills.remove(&name);
        }
    }
    if st.at_start == "cron" {
        let default = |name: &str| CRON_DEFAULTS.iter().find(|d| d.0 == name).map_or("", |d| d.1).to_string();
        let mut reset = false;
        while let Err((field, _)) = cron_expr(&st) {
            reset = true;
            match field {
                "cron_time" => st.cron_time = default("time"),
                "cron_days" => st.cron_days = default("days"),
                "cron_hours" => st.cron_hours = default("hours"),
                _ => st.cron_expr = default("expr"),
            }
        }
        if reset {
            changes.push("The repeat schedule was not valid; the invalid part was reset to its default.".into());
        }
        if matches!(st.at_stop.as_str(), "time" | "date") {
            changes.push("A recurring capture can only stop after a delay; changed to \"Stop manually\".".into());
            st.at_stop = "none".into();
            st.at_stop_value.clear();
        }
    }
    if (st.at_start == "cron" || st.at_script) && !valid_cron_name(&st.cron_name) {
        changes.push(format!("Job name {:?} changed to \"capture\".", st.cron_name));
        st.cron_name = "capture".into();
    }
    if at_spec(&st.at_start, &st.at_start_value, &st.at_start_unit).is_err() {
        changes.push("The start schedule is not valid; changed to \"Start now\".".into());
        st.at_start = "now".into();
        st.at_start_value.clear();
    }
    if at_spec(&st.at_stop, &st.at_stop_value, &st.at_stop_unit).is_err() {
        changes.push("The stop schedule is not valid; changed to \"Stop manually\".".into());
        st.at_stop = "none".into();
        st.at_stop_value.clear();
    }
    for (value, what) in [(&mut st.est_nodes, "Number of nodes"), (&mut st.est_ifaces, "Number of interfaces")] {
        if !value.is_empty() && !valid_count(value) {
            changes.push(format!("{what} for the estimate ({value:?}) cleared."));
            value.clear();
        }
    }
    if !cfg.estimate.link_speeds_gbit.iter().any(|s| s.to_string() == st.link_speed) {
        st.link_speed = cfg.estimate.default_link_speed_gbit.to_string();
        changes.push(format!("Link speed for the estimate set to {} Gbit/s.", st.link_speed));
    }
    if !st.free_space.is_empty() && !valid_free_space(&st.free_space) {
        changes.push(format!("Free space for the estimate ({:?}) cleared.", st.free_space));
        st.free_space.clear();
    }
    let mut probe = Ctx { issues: Vec::new() };
    check_raw_filter(st.raw.trim(), &mut probe);
    if !probe.issues.is_empty() {
        changes.push(format!("Extra filter expression {:?} removed.", st.raw));
        st.raw.clear();
    }

    Fixed { changes, output: write_command(cfg, st, Ctx { issues: Vec::new() }) }
}

pub fn build(cfg: &Config, pairs: &[(String, String)]) -> Output {
    let mut cx = Ctx { issues: Vec::new() };
    let st = resolve(cfg, pairs, &mut cx);
    write_command(cfg, st, cx)
}

/// Query string to form state, filling in defaults. Nothing is validated here
/// except the target id, which has to exist to pick the defaults.
fn resolve(cfg: &Config, pairs: &[(String, String)], cx: &mut Ctx) -> State {
    let q: HashMap<&str, &str> = pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let explicit = q.contains_key("v");
    let get = |k: &str| q.get(k).copied().unwrap_or("");
    let flag_on = |k: &str, default: bool| q.get(k).map_or(default, |v| *v == "1");

    let target = match q.get("t") {
        None => &cfg.targets[0],
        Some(id) => cfg.targets.iter().find(|t| t.id == *id).unwrap_or_else(|| {
            cx.error("target", format!("Unknown system {id:?}; showing {} instead.", cfg.targets[0].name));
            &cfg.targets[0]
        }),
    };

    let scope = match (get("sc"), &target.cluster) {
        (s @ ("all" | "nodes"), Some(_)) => s,
        _ => "node",
    };
    let can_all = target.all_interfaces.is_some() || target.any_interface.is_some();
    let iface_mode = if get("im") == "all" && can_all { "all" } else { "named" };
    let mut ifaces = split_list(get("if"));
    if ifaces.is_empty() {
        ifaces.push(target.default_interface.clone());
    }

    let or_default = |key: &str, name: &str| match get(key).trim() {
        "" => CRON_DEFAULTS.iter().find(|d| d.0 == name).map_or("", |d| d.1).to_string(),
        given => given.to_string(),
    };
    let mut st = State {
        target: target.id.clone(),
        scope: scope.into(),
        nodes: get("n").trim().to_string(),
        iface_mode: iface_mode.into(),
        ifaces,
        write: flag_on("w", true),
        subdir: get("sd").trim().trim_matches('/').to_string(),
        dated: flag_on("dd", target.dated_default),
        log: flag_on("lg", target.log_default),
        background: flag_on("bg", target.background_default),
        flags: resolve_flags(cfg, target, &q, explicit),
        hosts: split_list(get("h")),
        ports: split_list(get("p")),
        nets: split_list(get("net")),
        proto: get("pr").to_string(),
        raw: get("q").to_string(),
        at_start: match get("as") {
            m @ ("time" | "date" | "delay" | "cron") => m.to_string(),
            _ => "now".to_string(),
        },
        at_start_value: get("asv").trim().to_string(),
        at_start_unit: if get("asu").is_empty() { "hours".to_string() } else { get("asu").to_string() },
        at_stop: match get("ae") {
            m @ ("time" | "date" | "delay") => m.to_string(),
            _ => "none".to_string(),
        },
        at_stop_value: get("aev").trim().to_string(),
        at_stop_unit: if get("aeu").is_empty() { "hours".to_string() } else { get("aeu").to_string() },
        at_script: flag_on("asc", false),
        est_nodes: get("en").trim().to_string(),
        est_ifaces: get("ei").trim().to_string(),
        link_speed: match get("ls").trim() {
            "" => cfg.estimate.default_link_speed_gbit.to_string(),
            given => given.to_string(),
        },
        free_space: get("fs").trim().to_string(),
        cron_kind: match get("ck") {
            k @ ("weekdays" | "hours" | "custom") => k.to_string(),
            _ => "daily".to_string(),
        },
        cron_time: or_default("ct", "time"),
        cron_days: or_default("cd", "days"),
        cron_hours: or_default("ch", "hours"),
        cron_expr: or_default("cx", "expr"),
        cron_name: or_default("cn", "name"),
        fills: BTreeMap::new(),
    };
    // Keep only fill-in values whose placeholder is still in the settings.
    for (name, _) in placeholder_uses(&st) {
        if let Some(value) = q.get(format!("ph.{name}").as_str()).map(|v| v.trim()).filter(|v| !v.is_empty()) {
            st.fills.insert(name, value.to_string());
        }
    }
    st
}

pub const AT_UNITS: [&str; 3] = ["minutes", "hours", "days"];

fn two_digits(s: &str, max: u32) -> bool {
    s.len() == 2 && s.parse::<u32>().is_ok_and(|n| n <= max)
}

/// The time argument for `at`, or why the schedule is not usable.
/// `Ok(None)` means nothing is scheduled.
fn at_spec(mode: &str, value: &str, unit: &str) -> Result<Option<String>, String> {
    match mode {
        "time" => match value.split_once(':') {
            Some((h, m)) if two_digits(h, 23) && two_digits(m, 59) => Ok(Some(value.to_string())),
            _ => Err("Enter the time as HH:MM, for example 01:00.".into()),
        },
        "date" => {
            // YYYY-MM-DDTHH:MM, as a date-and-time input produces it.
            let b = value.as_bytes();
            let shaped = b.len() == 16 && b[4] == b'-' && b[7] == b'-' && b[10] == b'T' && b[13] == b':';
            let digits = |r: std::ops::Range<usize>| value.get(r).filter(|s| s.bytes().all(|c| c.is_ascii_digit()));
            match (shaped, digits(0..4), digits(5..7), digits(8..10), digits(11..13), digits(14..16)) {
                (true, Some(y), Some(mo), Some(d), Some(h), Some(mi))
                    if (1..=12).contains(&mo.parse().unwrap_or(0))
                        && (1..=31).contains(&d.parse().unwrap_or(0))
                        && two_digits(h, 23)
                        && two_digits(mi, 59) =>
                {
                    Ok(Some(format!("-t {y}{mo}{d}{h}{mi}")))
                }
                _ => Err("Pick a date and a time.".into()),
            }
        }
        "delay" => {
            if !AT_UNITS.contains(&unit) {
                return Err("Pick minutes, hours or days.".into());
            }
            match value.parse::<u32>() {
                Ok(n) if (1..=100_000).contains(&n) => Ok(Some(format!("now + {n} {unit}"))),
                _ => Err("Enter the delay as a whole number of 1 or more.".into()),
            }
        }
        _ => Ok(None),
    }
}

fn delay_minutes(value: &str, unit: &str) -> Option<u64> {
    let n: u64 = value.parse().ok()?;
    Some(n * match unit { "days" => 1440, "hours" => 60, _ => 1 })
}

fn human_size(bytes: u128) -> String {
    const UNITS: [(&str, f64); 4] = [("TB", 1e12), ("GB", 1e9), ("MB", 1e6), ("KB", 1e3)];
    let b = bytes as f64;
    let (unit, size) = UNITS.iter().find(|u| b >= u.1).unwrap_or(&UNITS[3]);
    let n = b / size;
    if n >= 100.0 { format!("{n:.0} {unit}") } else { format!("{n:.1} {unit}") }
}

fn human_duration(seconds: u128) -> String {
    let part = |n: u128, unit: &str| format!("{n} {unit}{}", if n == 1 { "" } else { "s" });
    let (d, h, m, s) = (seconds / 86400, seconds % 86400 / 3600, seconds % 3600 / 60, seconds % 60);
    let parts: Vec<String> = [(d, "day"), (h, "hour"), (m, "minute"), (s, "second")]
        .into_iter()
        .filter(|p| p.0 > 0)
        .map(|p| part(p.0, p.1))
        .collect();
    if parts.is_empty() { "0 seconds".into() } else { parts.join(" ") }
}

fn plural(n: u128, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// How many nodes "1,2,5-8" names.
fn count_nodes(list: &str) -> u128 {
    list.split(',')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('-').map(|(a, b)| (a.parse::<u128>(), b.parse::<u128>())) {
            Some((Ok(a), Ok(b))) if b >= a => b - a + 1,
            _ => 1,
        })
        .sum()
}

const MAX_COUNT: u128 = 100_000;

fn valid_count(s: &str) -> bool {
    s.parse::<u128>().is_ok_and(|n| (1..=MAX_COUNT).contains(&n))
}

fn valid_free_space(s: &str) -> bool {
    s.parse::<f64>().is_ok_and(|n| n.is_finite() && (0.0..=1e9).contains(&n)) && all_chars(s, ".")
}

fn check_estimate_inputs(cfg: &Config, st: &State, cx: &mut Ctx) {
    for (field, value, what) in [("est_nodes", &st.est_nodes, "number of nodes"), ("est_ifaces", &st.est_ifaces, "number of interfaces")] {
        if !value.is_empty() && !valid_count(value) {
            cx.error(field, format!("The {what} must be a whole number of 1 or more."));
        }
    }
    if !cfg.estimate.link_speeds_gbit.iter().any(|s| s.to_string() == st.link_speed) {
        cx.error("link_speed", "Pick one of the listed link speeds.");
    }
    if !st.free_space.is_empty() && !valid_free_space(&st.free_space) {
        cx.error("free_space", "Free space must be a number of gigabytes, for example 500.");
    }
}

/// Largest frame assumed for packet-count limits (jumbo frame plus headers).
const MAX_FRAME: u128 = 9018;
/// A standard full-size Ethernet frame, and what it occupies on the wire.
const FULL_FRAME: u128 = 1514;
const PCAP_RECORD: u128 = 16;

/// Link speed implied by an interface name such as `ix0`, from the config table.
fn interface_speed(cfg: &Config, name: &str) -> Option<u128> {
    cfg.estimate
        .interface_speeds
        .iter()
        .filter(|s| name.strip_prefix(s.prefix.as_str()).is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit())))
        .max_by_key(|s| s.prefix.len())
        .map(|s| s.gbit.into())
}

/// Upper bound on what the capture writes. `looped_all` is true when the
/// command loops over every interface, so their number is not in the command.
fn estimate(cfg: &Config, st: &State, looped_all: bool, cx: &mut Ctx) -> Estimate {
    if !st.write {
        return Estimate {
            kind: "none",
            headline: "No disk space used".into(),
            lines: vec!["Packets are printed to the screen, not written to a file.".into()],
            need_nodes: false,
            need_ifaces: false,
            need_link: false,
        };
    }
    let value = |role: Role| -> Option<u128> {
        let f = cfg.flags.iter().find(|f| f.role == Some(role))?;
        st.flags.get(f.key())?.parse().ok()
    };
    let name = |role: Role| cfg.flags.iter().find(|f| f.role == Some(role)).map_or("?", |f| f.name.as_str());
    let snap = value(Role::SnapLength).filter(|s| *s > 0);
    let (size_mb, count) = (value(Role::FileSizeMb), value(Role::FileCount));
    let (seconds, packets) = (value(Role::RotateSeconds), value(Role::PacketCount));
    let selected: u128 = st.link_speed.parse().unwrap_or(cfg.estimate.default_link_speed_gbit.into());

    // The limit for one capture on a link of `speed` Gbit/s: (bytes, kind).
    // Several limits can apply; the smallest wins.
    let timed = seconds.zip(count).filter(|_| size_mb.is_none()).map(|(secs, n)| secs * n);
    let limit = |speed: u128| -> Option<(u128, &'static str)> {
        let mut limits = Vec::new();
        if let (Some(mb), Some(n)) = (size_mb, count) {
            limits.push((mb * 1_000_000 * n, "exact"));
        } else if let Some(duration) = timed {
            limits.push((speed * 1_000_000_000 / 8 * 2 * duration, "worst"));
        }
        if let Some(n) = packets {
            limits.push((n * (snap.unwrap_or(MAX_FRAME).min(MAX_FRAME) + PCAP_RECORD) + 24, "exact"));
        }
        limits.into_iter().min_by_key(|l| l.0)
    };

    // One entry per capture running on a node: (interface, speed, speed came from the name).
    let streams: Option<Vec<(String, u128, bool)>> = match (st.iface_mode.as_str(), looped_all) {
        ("all", true) => st.est_ifaces.parse::<usize>().ok().map(|n| vec![(String::new(), selected, false); n]),
        ("all", false) => Some(vec![(String::new(), selected, false)]),
        _ => Some(
            st.ifaces
                .iter()
                .map(|i| match interface_speed(cfg, i) {
                    Some(speed) => (i.clone(), speed, true),
                    None => (i.clone(), selected, false),
                })
                .collect(),
        ),
    };
    let nodes: Option<u128> = match st.scope.as_str() {
        "nodes" => Some(count_nodes(&st.nodes)),
        "all" => st.est_nodes.parse().ok(),
        _ => Some(1),
    };
    let need_nodes = st.scope == "all";
    let free: Option<u128> = st.free_space.parse::<f64>().ok().map(|gb| (gb * 1e9) as u128);

    let fastest = streams.iter().flatten().map(|s| s.1).max().unwrap_or(selected);
    let Some((per_iface, kind)) = limit(fastest) else {
        let reason = match (size_mb, seconds) {
            (Some(mb), _) => format!("{} without {} keeps creating {mb} MB files until the capture is stopped.", name(Role::FileSizeMb), name(Role::FileCount)),
            (None, Some(s)) => format!("{} without {} starts a new file every {} and never stops.", name(Role::RotateSeconds), name(Role::FileCount), human_duration(s)),
            _ => "Nothing rotates or limits the file, so it grows until the capture is stopped.".to_string(),
        };
        cx.warn("estimate", format!("This capture has no size limit and can fill the drive. {reason} Add a file count or a packet count to cap it."));
        return Estimate { kind: "unbounded", headline: "No upper limit".into(), lines: vec![reason], need_nodes: false, need_ifaces: false, need_link: false };
    };
    let per_node: Option<u128> = streams.as_ref().map(|list| list.iter().filter_map(|s| limit(s.1)).map(|l| l.0).sum());
    let total = per_node.zip(nodes).map(|(p, n)| p * n);
    let from_dropdown = streams.as_ref().is_none_or(|list| list.iter().any(|s| !s.2));

    // Explain the figure.
    let mut lines: Vec<String> = Vec::new();
    match (kind, timed) {
        ("worst", Some(duration)) => {
            lines.push(format!(
                "The capture runs for {} ({} × {}). The size depends on the traffic; this assumes every link completely full in both directions the whole time, so real captures are normally far smaller.",
                human_duration(duration), name(Role::RotateSeconds), name(Role::FileCount)
            ));
            if let Some(s) = snap.filter(|s| *s < FULL_FRAME) {
                let (whole, per) = match per_node {
                    Some(p) => (p, "per node"),
                    None => (per_iface, "per interface"),
                };
                lines.push(format!(
                    "If that traffic is mostly full-size 1500-byte frames, the {s}-byte snap length cuts it to about {} {per}.",
                    human_size(whole * (s + PCAP_RECORD) / (FULL_FRAME + 20))
                ));
            }
            let named: Vec<String> = streams.iter().flatten().filter(|s| s.2).map(|s| format!("{} {} Gbit/s", s.0, s.1)).collect();
            let unnamed: Vec<&str> = streams.iter().flatten().filter(|s| !s.2 && !s.0.is_empty()).map(|s| s.0.as_str()).collect();
            if !named.is_empty() {
                lines.push(format!("Link speed taken from the interface name: {}.", named.join(", ")));
            }
            if from_dropdown {
                let whom = if unnamed.is_empty() { String::new() } else { format!(" for {}", unnamed.join(", ")) };
                lines.push(format!("Using the link speed selected below ({selected} Gbit/s){whom}."));
            }
        }
        _ if packets.is_some() && limit(fastest).map(|l| l.0) != size_mb.zip(count).map(|(mb, n)| mb * 1_000_000 * n) => {
            let n = packets.unwrap_or(0);
            let per_packet = snap.unwrap_or(MAX_FRAME).min(MAX_FRAME) + PCAP_RECORD;
            lines.push(format!("The capture stops after {} ({}) of at most {per_packet} bytes each.", plural(n, "packet"), name(Role::PacketCount)));
        }
        _ => {
            let (mb, n) = (size_mb.unwrap_or(0), count.unwrap_or(0));
            lines.push(format!(
                "{} of {mb} MB per interface ({} × {}). The oldest file is overwritten, so this is a hard limit; the capture runs until it is stopped.",
                plural(n, "file"), name(Role::FileSizeMb), name(Role::FileCount)
            ));
        }
    }

    let prefix = if kind == "worst" { "Worst case" } else { "Up to" };
    let headline = match (total, per_node, nodes) {
        (Some(t), _, _) => format!("{prefix} {}", human_size(t)),
        (None, Some(p), _) => format!("{prefix} {} per node", human_size(p)),
        (None, None, Some(n)) => format!("{prefix} {} per interface", human_size(per_iface * n)),
        _ => format!("{prefix} {} per interface, per node", human_size(per_iface)),
    };
    match (streams.as_ref(), nodes) {
        (Some(list), Some(n)) => {
            let i = list.len() as u128;
            let uniform = list.iter().all(|s| limit(s.1) == limit(fastest));
            if uniform && n * i > 1 {
                lines.push(format!("{} per interface × {} × {}.", human_size(per_iface), plural(i, "interface"), plural(n, "node")));
            } else if !uniform && n > 1 {
                lines.push(format!("{} per node × {}.", human_size(per_node.unwrap_or(0)), plural(n, "node")));
            }
        }
        (list, n) => {
            let missing = match (list.is_none(), n.is_none()) {
                (true, true) => "the number of nodes and of interfaces per node",
                (false, true) => "the number of nodes",
                _ => "the number of interfaces per node",
            };
            lines.push(format!("Enter {missing} below to get the total."));
        }
    }
    if let Some(free) = free {
        let compared = total.or(per_node).unwrap_or(per_iface);
        if compared > free {
            let what = if kind == "worst" { "The worst case" } else { "The capture" };
            let scope = if total.is_some() { "" } else { ", before multiplying by the missing counts" };
            cx.warn(
                "free_space",
                format!("{what} ({}{scope}) is larger than the {} GB of free space entered, so it can fill the drive.", human_size(compared), st.free_space),
            );
        }
    }
    Estimate { kind, headline, lines, need_nodes, need_ifaces: looped_all && st.iface_mode == "all", need_link: kind == "worst" && from_dropdown }
}

/// "a", "a and b", "a, b and c".
fn word_list(items: &[String], last: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., end] => format!("{} {last} {end}", rest.join(", ")),
    }
}

/// When an `at` job runs, in words. `None` when nothing is scheduled that way.
fn when_in_words(mode: &str, value: &str, unit: &str) -> Option<String> {
    match mode {
        "time" => Some(format!("at {value}")),
        "date" => value.split_once('T').map(|(day, time)| format!("on {day} at {time}")),
        "delay" => {
            let unit = if value == "1" { unit.trim_end_matches('s') } else { unit };
            Some(format!("{value} {unit} after the command is pasted"))
        }
        _ => None,
    }
}

/// Explain the command as configured, in a few short sentences.
fn summarize(cfg: &Config, st: &State, target: &Target, dir: &str, background: bool, stop_delay: Option<u64>) -> Vec<String> {
    let value = |role: Role| -> Option<u128> {
        let f = cfg.flags.iter().find(|f| f.role == Some(role))?;
        st.flags.get(f.key())?.parse().ok()
    };
    let mut lines = Vec::new();

    // Where.
    let ifaces = if st.iface_mode == "all" { "every interface".to_string() } else { word_list(&st.ifaces, "and") };
    let nodes = match st.scope.as_str() {
        "all" => " on every node".to_string(),
        "nodes" => format!(" on node{} {}", if st.nodes.contains([',', '-', '<']) { "s" } else { "" }, st.nodes),
        _ if target.cluster.is_some() => " on this node".to_string(),
        _ => String::new(),
    };
    lines.push(format!("Captures on {ifaces}{nodes}."));

    // What traffic.
    let mut which = Vec::new();
    let addresses: Vec<String> = st.hosts.iter().cloned().chain(st.nets.iter().map(|n| format!("network {n}"))).collect();
    if !addresses.is_empty() {
        which.push(format!("to or from {}", word_list(&addresses, "or")));
    }
    if !st.ports.is_empty() {
        which.push(format!("on port{} {}", if st.ports.len() > 1 || st.ports[0].contains('-') { "s" } else { "" }, word_list(&st.ports, "or")));
    }
    if !st.proto.is_empty() {
        which.push(format!("using {}", st.proto));
    }
    if !st.raw.trim().is_empty() {
        which.push(format!("matching \"{}\"", st.raw.trim()));
    }
    lines.push(if which.is_empty() { "All traffic is captured; there is no filter.".to_string() } else { format!("Only traffic {}.", which.join(", and ")) });

    // What is kept, and how the files are limited.
    let kept = match value(Role::SnapLength) {
        Some(n) if n > 0 => format!("the first {n} bytes of each packet"),
        _ => "whole packets".to_string(),
    };
    let (size_mb, count) = (value(Role::FileSizeMb), value(Role::FileCount));
    let (seconds, packets) = (value(Role::RotateSeconds), value(Role::PacketCount));
    if st.write {
        lines.push(format!("Saves {kept} to {dir}."));
        lines.push(match (size_mb, seconds, count) {
            (Some(mb), _, Some(n)) => format!("Keeps the newest {} of {mb} MB per interface, overwriting the oldest, until it is stopped.", plural(n, "file")),
            (Some(mb), _, None) => format!("Starts a new {mb} MB file each time one fills, with no limit on how many."),
            (None, Some(s), Some(n)) => format!("Starts a new file every {} and ends by itself after {} ({}).", human_duration(s), plural(n, "file"), human_duration(s * n)),
            (None, Some(s), None) => format!("Starts a new file every {} and keeps going until it is stopped.", human_duration(s)),
            _ => "Writes one file per interface, which grows until the capture is stopped.".to_string(),
        });
    } else {
        lines.push("Prints packets to the screen; nothing is saved.".to_string());
    }
    if let Some(n) = packets {
        lines.push(format!("Stops after {}.", plural(n, "packet")));
    }
    match (background, st.write && st.log) {
        (true, true) => lines.push("Runs in the background, with tcpdump's own messages saved to a .log file beside the capture.".into()),
        (true, false) => lines.push("Runs in the background.".into()),
        (false, true) => lines.push("tcpdump's own messages are saved to a .log file beside the capture.".into()),
        _ => {}
    }

    // Other options, by their labels.
    let extras: Vec<String> = cfg
        .flags
        .iter()
        .filter(|f| f.role.is_none() && st.flags.contains_key(f.key()))
        .map(|f| {
            let label = if f.label.is_empty() { f.name.clone() } else { format!("{} ({})", f.label.to_lowercase(), f.name) };
            match (f.value_type, st.flags[f.key()].as_str()) {
                (ValueType::Bool, _) | (_, "") => label,
                (_, v) => format!("{label} set to {v}"),
            }
        })
        .collect();
    if !extras.is_empty() {
        lines.push(format!("Also: {}.", word_list(&extras, "and")));
    }

    // When.
    if st.at_start == "cron" {
        if let Ok(expr) = cron_expr(st) {
            let via = "Installed as a cron job that runs a script.";
            lines.push(format!("{} {via}", describe_cron(&expr)));
        }
        if let Some(minutes) = stop_delay {
            lines.push(format!("Each run is stopped after {}.", human_duration(u128::from(minutes) * 60)));
        }
    } else {
        if let Some(when) = when_in_words(&st.at_start, &st.at_start_value, &st.at_start_unit) {
            let how = if st.at_script { "queued with at from a script file" } else { "queued with at" };
            lines.push(format!("Starts {when} ({how}), not straight away."));
        }
        if let Some(when) = when_in_words(&st.at_stop, &st.at_stop_value, &st.at_stop_unit) {
            lines.push(format!("A stop is queued for {when}."));
        }
    }
    lines
}

fn target_of<'a>(cfg: &'a Config, st: &State) -> &'a Target {
    cfg.targets.iter().find(|t| t.id == st.target).unwrap_or(&cfg.targets[0])
}

/// Validate the state and write the command.
fn write_command(cfg: &Config, raw: State, mut cx: Ctx) -> Output {
    // Placeholders: check what was filled in, then build from the settings
    // with the usable values in place. `raw` keeps the placeholders for the form.
    let uses = placeholder_uses(&raw);
    let mut usable = raw.fills.clone();
    for (name, kind) in &uses {
        if let Some(problem) = raw.fills.get(name).and_then(|v| fill_problem(v, *kind)) {
            if usable.remove(name).is_some() {
                cx.error(&format!("ph.{name}"), problem);
            }
        }
    }
    let mut placeholders: Vec<Placeholder> = Vec::new();
    for (name, kind) in &uses {
        if !placeholders.iter().any(|p| &p.name == name) {
            placeholders.push(Placeholder { name: name.clone(), hint: kind.label(), value: raw.fills.get(name).cloned().unwrap_or_default() });
        }
    }
    let unfilled: Vec<&str> = placeholders.iter().filter(|p| !usable.contains_key(&p.name)).map(|p| p.name.as_str()).collect();
    if !unfilled.is_empty() {
        cx.warn("placeholders", format!("Fill in {} before running this command.", unfilled.join(", ")));
    }
    let st = with_fills(&raw, &usable);
    let target = target_of(cfg, &st);
    let scope = st.scope.clone();
    let scope = scope.as_str();

    // Interfaces: a literal name, or a shell loop over several.
    let loop_list: Option<String> = if st.iface_mode == "all" {
        target.all_interfaces.clone().filter(|_| target.any_interface.is_none())
    } else {
        for i in &st.ifaces {
            if !valid_iface(i) {
                cx.error("ifaces", format!("{i:?} is not a valid interface name."));
            }
        }
        (st.ifaces.len() > 1).then(|| st.ifaces.join(" "))
    };
    let iface: String = match (&loop_list, st.iface_mode.as_str()) {
        (Some(_), _) => "${iface}".into(),
        (None, "all") => target.any_interface.clone().unwrap_or_default(),
        (None, _) => st.ifaces[0].clone(),
    };

    // Flags, in the order the config lists them.
    let enabled = |name: &str| st.flags.contains_key(name.trim_start_matches('-'));
    let mut cmd = format!("{} -i {iface}", target.command);
    for f in cfg.flags.iter().filter(|f| st.flags.contains_key(f.key())) {
        let value = &st.flags[f.key()];
        check_flag(f, value, &enabled, &mut cx);
        cmd.push(' ');
        cmd.push_str(&f.name);
        if f.value_type != ValueType::Bool {
            cmd.push(' ');
            cmd.push_str(value);
        }
    }

    // Output file, log and backgrounding only apply when writing to disk.
    let mut dir = target.output_dir.clone();
    if !st.subdir.is_empty() {
        if !valid_subdir(&st.subdir) {
            cx.error("subdir", "The subfolder may only contain letters, digits and _ . - /");
        }
        dir = format!("{dir}/{}", st.subdir);
    }
    if st.dated {
        dir = format!("{dir}/{}", target.date_subdir);
    }
    let background = st.write && st.background;
    if st.write {
        let timestamp = if target.strftime_flags.iter().any(|f| enabled(f)) {
            &target.strftime_timestamp
        } else {
            &target.timestamp
        };
        let vars = [("hostname", target.hostname.as_str()), ("timestamp", timestamp.as_str()), ("iface", iface.as_str())];
        cmd.push_str(&format!(" -w {dir}/{}", render(&target.file_name, &vars)));
    } else if scope != "node" || loop_list.is_some() {
        cx.warn("write", "Printing packets from several interfaces or nodes at once is hard to read; consider writing to a file.");
    }

    let filter = build_filter(cfg, &st, &mut cx);
    if !filter.is_empty() {
        cmd.push(' ');
        cmd.push_str(&filter);
    }
    if st.write && st.log {
        let vars = [("hostname", target.hostname.as_str()), ("iface", iface.as_str())];
        let log = format!("{dir}/{}", render(&target.log_name, &vars));
        cmd.push(' ');
        // An `at` job runs under /bin/sh, which does not understand `&>>`.
        let redirect = if st.at_start == "now" { &target.log_redirect } else { &target.at_log_redirect };
        cmd.push_str(&render(redirect, &[("log", &log), ("dir", &dir)]));
    }
    if background {
        cmd.push_str(" &");
    } else if st.write && loop_list.is_some() {
        cx.warn("background", "Without \"run in background\" the loop captures one interface at a time, moving on only when each capture ends.");
    }
    if let Some(list) = &loop_list {
        let sep = if background { "" } else { ";" };
        cmd = format!("for iface in {list}; do {cmd}{sep} done");
    }

    // Cluster wrapper.
    let wrap = |inner: &str, cx: &mut Ctx| -> String {
        let Some(cluster) = &target.cluster else { return inner.to_string() };
        if scope != "node" && inner.contains('\'') {
            cx.error("scope", "This command contains a single quote and cannot be wrapped for the cluster.");
        }
        match scope {
            "all" => render(&cluster.all_nodes, &[("command", inner)]),
            "nodes" => render(&cluster.selected_nodes, &[("command", inner), ("nodes", &st.nodes)]),
            _ => inner.to_string(),
        }
    };
    if scope == "nodes" && !valid_nodes(&st.nodes) {
        cx.error("nodes", "List the node numbers, e.g. 1,2,5 or 1-4.");
    }
    cmd = wrap(&cmd, &mut cx);
    if st.write {
        cmd = format!("{}; {cmd}", render(&target.prepare, &[("dir", &dir)]));
    }

    // Scheduling: wrap the capture, and optionally the stop command, in `at` jobs.
    let start_spec = at_spec(&st.at_start, &st.at_start_value, &st.at_start_unit).unwrap_or_else(|e| {
        cx.error("at_start_value", e);
        None
    });
    let is_cron = st.at_start == "cron";
    let stop_spec = at_spec(&st.at_stop, &st.at_stop_value, &st.at_stop_unit).unwrap_or_else(|e| {
        cx.error("at_stop_value", e);
        None
    });
    // A recurring capture has no `at` jobs; its stop is a delay inside the script.
    let stop_delay = stop_spec.as_ref().filter(|_| is_cron).and_then(|_| delay_minutes(&st.at_stop_value, &st.at_stop_unit));
    if is_cron && stop_spec.is_some() && st.at_stop != "delay" {
        cx.error("at_stop_value", "A recurring capture can only be stopped by a delay after each start.");
    }
    let stop_spec = stop_spec.filter(|_| !is_cron);
    let stop_plain = wrap(&target.stop, &mut cx);
    let job = |spec: &str, body: &str| format!("{} {spec} <<'EOF'\n{body}\nEOF", target.at_command);
    // Scripts (for a recurring capture, or an `at` job queued from a file).
    let script_dir = target.cron_dir.clone().unwrap_or_else(|| target.output_dir.clone());
    let script = format!("{script_dir}/tcpdump_{}.sh", st.cron_name);
    let uses_script = is_cron || (st.at_script && start_spec.is_some());
    if uses_script && !valid_cron_name(&st.cron_name) {
        cx.error("cron_name", "Use only letters, digits, _ and - for the job name.");
    }
    if let Some(spec) = &start_spec {
        cmd = if st.at_script {
            render(&target.at_script_install, &[("dir", &script_dir), ("script", &script), ("command", &cmd), ("at", &target.at_command), ("spec", spec)])
        } else {
            job(spec, &cmd)
        };
        if !st.write {
            cx.warn("at_start_value", "A scheduled capture has no screen to print to; turn on \"Write packets to a file\".");
        }
    }
    if let Some(spec) = &stop_spec {
        let stop_job = job(spec, &stop_plain);
        // When the capture starts right away, queue the stop first so it is
        // in place even if the capture runs in the foreground.
        cmd = if start_spec.is_some() { format!("{cmd}\n{stop_job}") } else { format!("{stop_job}\n{cmd}") };
        let late = match (st.at_start.as_str(), st.at_stop.as_str()) {
            ("date", "date") => st.at_stop_value <= st.at_start_value,
            ("delay", "delay") => delay_minutes(&st.at_stop_value, &st.at_stop_unit) <= delay_minutes(&st.at_start_value, &st.at_start_unit),
            _ => false,
        };
        if late {
            cx.warn("at_stop_value", "The stop is scheduled at or before the start, so the capture would not be stopped.");
        }
    }
    let mut manage: Vec<Manage> = Vec::new();
    let mut schedule_summary = None;
    if start_spec.is_some() || stop_spec.is_some() {
        manage.push(Manage { label: "List the queued jobs and their numbers", command: target.at_list.clone() });
        manage.push(Manage { label: "Show what a queued job will run", command: target.at_show.clone() });
        manage.push(Manage { label: "Remove a queued job", command: target.at_remove.clone() });
        if st.at_script && start_spec.is_some() {
            manage.push(Manage { label: "Delete the script once the job has run", command: render(&target.script_remove, &[("script", &script)]) });
        }
    }
    if is_cron {
        let expr = cron_expr(&st).unwrap_or_else(|(field, message)| {
            cx.error(field, message);
            String::new()
        });
        // The stop timer goes first so it is running even if the capture stays in the foreground.
        let body = match stop_delay {
            Some(minutes) => format!("(sleep {}; {stop_plain}) &\n{cmd}", minutes * 60),
            None => cmd.clone(),
        };
        cmd = render(&target.cron_install, &[("dir", &script_dir), ("script", &script), ("expr", &expr), ("command", &body)]);
        if !st.write {
            cx.warn("at_start_value", "A scheduled capture has no screen to print to; turn on \"Write packets to a file\".");
        }
        let role = |r: Role| cfg.flags.iter().find(|f| f.role == Some(r)).is_some_and(|f| st.flags.contains_key(f.key()));
        let ends = role(Role::PacketCount) || (role(Role::RotateSeconds) && role(Role::FileCount) && !role(Role::FileSizeMb));
        if !ends && stop_delay.is_none() {
            cx.warn("cron", "This capture does not end by itself, so every run would start another one on top of it. Rotate by time with a file count, set a packet count, or add a stop delay.");
        }
        if st.write {
            cx.warn("cron", "A recurring capture adds files on every run and can fill the drive over time. Delete old captures regularly, and remove the cron job when you are done.");
        }
        if !expr.is_empty() {
            schedule_summary = Some(describe_cron(&expr));
        }
        manage.push(Manage { label: "List the cron jobs", command: target.cron_list.clone() });
        manage.push(Manage { label: "Remove this recurring capture", command: render(&target.cron_remove, &[("script", &script)]) });
    }
    let stop_command = (background || is_cron || start_spec.is_some() || stop_spec.is_some()).then_some(stop_plain);

    check_estimate_inputs(cfg, &st, &mut cx);
    let failed = cx.issues.iter().any(|i| i.level == "error");
    let estimate = (!failed).then(|| estimate(cfg, &st, loop_list.is_some() && st.iface_mode == "all", &mut cx));
    Output {
        command: if failed { String::new() } else { cmd },
        stop_command: if failed { None } else { stop_command },
        issues: cx.issues,
        estimate,
        placeholders,
        summary: if failed { Vec::new() } else { summarize(cfg, &st, target, &dir, background, stop_delay) },
        schedule_summary: schedule_summary.filter(|_| !failed),
        manage: if failed { Vec::new() } else { manage },
        state: raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::parse(include_str!("../tcpdump.toml")).expect("shipped config is valid")
    }

    fn run(query: &str) -> Output {
        build(&cfg(), &parse_query(query))
    }

    fn errors(o: &Output) -> Vec<&str> {
        o.issues.iter().filter(|i| i.level == "error").map(|i| i.message.as_str()).collect()
    }

    #[test]
    fn defaults_build_cleanly_for_every_target() {
        let cfg = cfg();
        for t in &cfg.targets {
            let o = build(&cfg, &parse_query(&format!("t={}", t.id)));
            assert!(errors(&o).is_empty(), "{}: {:?}", t.id, o.issues);
            assert!(o.command.contains("tcpdump -i "));
        }
    }

    #[test]
    fn timed_single_interface() {
        let o = run("v=1&t=powerscale&if=lagg0&w=1&dd=1&lg=1&bg=1&f.s=600&f.G=3600&f.W=4&h=192.168.1.1");
        assert_eq!(
            o.command,
            "mkdir -p /ifs/data/Isilon_Support/$(date +%F); tcpdump -i lagg0 -s 600 -G 3600 -W 4 \
             -w /ifs/data/Isilon_Support/$(date +%F)/$(uname -n).%F_%H%M%S.lagg0.pcap host 192.168.1.1 \
             &>> /ifs/data/Isilon_Support/$(date +%F)/$(uname -n).$(date +%F_%H%M%S).lagg0.log &"
        );
        assert_eq!(o.stop_command.as_deref(), Some("killall -SIGINT tcpdump"));
    }

    #[test]
    fn cluster_loop_with_grouped_filter() {
        let o = run("v=1&t=powerscale&sc=nodes&n=1,2&if=bxe0,bxe1&sd=NDMP&w=1&dd=1&lg=0&bg=1&f.s=900&f.C=1024&f.W=10&h=10.0.0.1,10.0.0.2&p=445,2049");
        assert_eq!(
            o.command,
            "mkdir -p /ifs/data/Isilon_Support/NDMP/$(date +%F); isi_for_array -n1,2 'for iface in bxe0 bxe1; do \
             tcpdump -i ${iface} -s 900 -C 1024 -W 10 \
             -w /ifs/data/Isilon_Support/NDMP/$(date +%F)/$(uname -n).$(date +%F_%H%M%S).${iface}.pcap \
             \"( host 10.0.0.1 or host 10.0.0.2 ) and ( port 445 or port 2049 )\" & done'"
        );
        assert_eq!(o.stop_command.as_deref(), Some("isi_for_array -n1,2 'killall -SIGINT tcpdump'"));
    }

    #[test]
    fn linux_live_capture_uses_any() {
        let o = run("v=1&t=linux&im=all&w=0&f.nn=1&p=53");
        assert_eq!(o.command, "sudo tcpdump -i any -nn port 53");
        assert!(o.stop_command.is_none());
    }

    #[test]
    fn absent_flags_are_off_once_state_is_explicit() {
        let o = run("v=1&t=linux&w=0");
        assert!(o.state.flags.is_empty());
        assert_eq!(o.command, "sudo tcpdump -i eth0");
    }

    #[test]
    fn dependencies_exclusions_and_ranges() {
        assert!(!errors(&run("v=1&f.W=4")).is_empty());
        assert!(!errors(&run("v=1&f.C=10&f.G=60")).is_empty());
        assert!(!errors(&run("v=1&f.s=99999999")).is_empty());
        assert!(!errors(&run("v=1&f.s=abc")).is_empty());
    }

    #[test]
    fn shell_injection_is_rejected_everywhere() {
        for q in [
            "if=eth0;reboot",
            "if=$(reboot)",
            "sd=x;reboot",
            "sd=../../etc",
            "sc=nodes&n=1;reboot",
            "h=1.1.1.1;reboot",
            "p=53;reboot",
            "net=10.0.0.0/8`id`",
            "q=port 53; reboot",
            "q=port 53\" ; reboot ; \"",
            "q=host $(id)",
            "q=port 53' ; reboot '",
            "q=!tcp",
            "pr=tcp;id",
            "v=1&f.s=1;id",
            "v=1&t=linux&f.Z=root;id",
            "t=nope",
        ] {
            let o = run(q);
            assert!(!errors(&o).is_empty(), "{q} should be rejected");
            assert!(o.command.is_empty(), "{q} must not yield a command");
            assert!(o.stop_command.is_none());
        }
    }

    fn warnings(o: &Output) -> Vec<&str> {
        o.issues.iter().filter(|i| i.level == "warning").map(|i| i.message.as_str()).collect()
    }

    #[test]
    fn estimate_for_a_ring_buffer_is_exact() {
        // 10 files of 1024 MB on 2 interfaces of this node.
        let o = run("v=1&if=bxe0,bxe1&w=1&bg=1&f.s=600&f.C=1024&f.W=10");
        let e = o.estimate.as_ref().unwrap();
        assert_eq!((e.kind, e.headline.as_str()), ("exact", "Up to 20.5 GB"));
        assert!(!e.need_nodes && !e.need_ifaces && !e.need_link);
        assert!(warnings(&o).is_empty(), "{:?}", o.issues);
        // Named nodes are counted from the list, ranges included.
        let o = run("v=1&sc=nodes&n=1,2,5-8&if=lagg1&w=1&bg=1&f.C=1000&f.W=100");
        assert_eq!(o.estimate.unwrap().headline, "Up to 600 GB");
    }

    #[test]
    fn estimate_asks_for_counts_the_command_cannot_know() {
        let q = "v=1&sc=all&im=all&w=1&bg=1&f.C=500&f.W=5";
        let e = run(q).estimate.unwrap();
        assert!(e.need_nodes && e.need_ifaces);
        assert_eq!(e.headline, "Up to 2.5 GB per interface, per node");
        assert!(e.lines.last().unwrap().contains("number of nodes and of interfaces"));
        let e = run(&format!("{q}&en=4")).estimate.unwrap();
        assert_eq!(e.headline, "Up to 10.0 GB per interface");
        let e = run(&format!("{q}&en=4&ei=3")).estimate.unwrap();
        assert_eq!(e.headline, "Up to 30.0 GB");
        // Linux "any" is one capture, so nothing needs to be asked.
        let e = run("v=1&t=linux&im=all&w=1&f.C=500&f.W=5").estimate.unwrap();
        assert!(!e.need_ifaces && e.headline == "Up to 2.5 GB");
    }

    #[test]
    fn estimate_for_a_timed_capture_is_the_full_link_worst_case() {
        // 4 hours at 10 Gbit/s, both directions: 2.5 GB/s x 14400 s = 36 TB.
        let o = run("v=1&if=lagg0&w=1&f.s=600&f.G=3600&f.W=4");
        let e = o.estimate.as_ref().unwrap();
        assert_eq!((e.kind, e.headline.as_str(), e.need_link), ("worst", "Worst case 36.0 TB", true));
        assert!(e.lines[0].contains("4 hours") && e.lines[1].contains("600-byte snap length"));
        let e = run("v=1&if=lagg0&w=1&f.s=0&f.G=300&f.W=1&ls=1").estimate.unwrap();
        assert_eq!(e.headline, "Worst case 75.0 GB");
        assert_eq!(e.lines.len(), 2);
        assert_eq!(e.lines[1], "Using the link speed selected below (1 Gbit/s) for lagg0.");
        // A packet count caps it exactly, and the smaller limit wins.
        let e = run("v=1&if=lagg0&w=1&f.s=600&f.G=3600&f.W=4&f.c=1000").estimate.unwrap();
        assert_eq!((e.kind, e.headline.as_str()), ("exact", "Up to 616 KB"));
    }

    #[test]
    fn link_speed_comes_from_the_interface_name_when_it_is_recognised() {
        // ix0 is 10 Gbit/s and mlxen0 40 Gbit/s: 50 Gbit/s both ways for an hour is 45 TB.
        let e = run("v=1&if=ix0,mlxen0&w=1&f.s=0&f.G=3600&f.W=1&ls=1").estimate.unwrap();
        assert_eq!((e.headline.as_str(), e.need_link), ("Worst case 45.0 TB", false));
        assert!(e.lines.iter().any(|l| l == "Link speed taken from the interface name: ix0 10 Gbit/s, mlxen0 40 Gbit/s."));
        assert!(!e.lines.iter().any(|l| l.contains("selected below")));
        // An unrecognised name falls back to the dropdown, which is then shown.
        let e = run("v=1&if=ix0,lagg0&w=1&f.s=0&f.G=3600&f.W=1&ls=25").estimate.unwrap();
        assert_eq!((e.headline.as_str(), e.need_link), ("Worst case 31.5 TB", true));
        assert!(e.lines.iter().any(|l| l == "Using the link speed selected below (25 Gbit/s) for lagg0."));
        // "ixl0" is not "ix" plus a number, and a filled-in placeholder is read like a typed name.
        assert!(run("v=1&if=ixl0&w=1&f.G=3600&f.W=1").estimate.unwrap().need_link);
        let e = run("v=1&sc=nodes&n=1-4&if=<interface>&w=1&f.s=0&f.G=3600&f.W=1&ph.<interface>=em0,em1").estimate.unwrap();
        assert_eq!(e.headline, "Worst case 7.2 TB");
        assert!(e.lines.iter().any(|l| l == "900 GB per interface × 2 interfaces × 4 nodes."));
        // Exact limits do not depend on speed.
        assert_eq!(run("v=1&if=ix0,mlxen0&w=1&f.C=1000&f.W=10").estimate.unwrap().headline, "Up to 20.0 GB");
    }

    #[test]
    fn at_job_can_be_queued_from_a_script() {
        let o = run("v=1&if=lagg0&w=1&dd=1&lg=1&bg=1&f.s=600&f.C=500&f.W=5&as=time&asv=01:00&asc=1&cn=case42");
        assert!(errors(&o).is_empty(), "{:?}", o.issues);
        let lines: Vec<&str> = o.command.lines().collect();
        assert_eq!(lines[0], "mkdir -p /ifs/data/Isilon_Support");
        assert_eq!((lines[1], lines[2]), ("cat > /ifs/data/Isilon_Support/tcpdump_case42.sh <<'EOF'", "#!/bin/sh"));
        assert!(lines[3].contains("tcpdump -i lagg0") && lines[3].contains(".log 2>&1 &"));
        assert_eq!(lines[4..], ["EOF", "chmod +x /ifs/data/Isilon_Support/tcpdump_case42.sh", "at -f /ifs/data/Isilon_Support/tcpdump_case42.sh 01:00"]);
        assert_eq!(o.manage.last().unwrap().command, "rm -f /ifs/data/Isilon_Support/tcpdump_case42.sh");
        let o = run("v=1&t=linux&w=1&f.C=500&f.W=5&as=delay&asv=2&asu=hours&asc=1&ae=time&aev=05:00");
        assert!(o.command.contains("\nsudo at -f /usr/local/sbin/tcpdump_capture.sh now + 2 hours\nsudo at 05:00 <<'EOF'\n"));
        // The toggle does nothing when the capture starts now, and a bad name is caught.
        assert!(!run("v=1&w=1&f.C=500&f.W=5&asc=1").command.contains("at -f"));
        assert!(!errors(&run("v=1&w=1&f.C=500&f.W=5&as=time&asv=01:00&asc=1&cn=a;b")).is_empty());
        assert!(errors(&fix(&cfg(), &parse_query("v=1&w=1&f.C=500&f.W=5&as=time&asv=01:00&asc=1&cn=a;b")).output).is_empty());
    }

    #[test]
    fn warns_when_nothing_caps_the_capture_or_it_exceeds_free_space() {
        for q in ["v=1&w=1", "v=1&w=1&f.s=600", "v=1&w=1&f.C=1024", "v=1&w=1&f.G=3600"] {
            let o = run(q);
            assert_eq!(o.estimate.as_ref().unwrap().kind, "unbounded", "{q}");
            assert!(warnings(&o).iter().any(|w| w.contains("can fill the drive")), "{q}");
            assert!(!o.command.is_empty());
        }
        let q = "v=1&if=bxe0,bxe1&w=1&bg=1&f.C=1024&f.W=10";
        assert!(warnings(&run(&format!("{q}&fs=50"))).is_empty());
        let o = run(&format!("{q}&fs=20"));
        assert!(warnings(&o)[0].contains("larger than the 20 GB of free space"));
        // Printing to the screen uses no disk and never warns.
        let o = run("v=1&t=linux&w=0&fs=1");
        assert_eq!(o.estimate.as_ref().unwrap().kind, "none");
        assert!(warnings(&o).is_empty());
    }

    #[test]
    fn estimate_inputs_are_validated_and_fixable() {
        for q in ["en=0", "en=abc", "ei=-1", "ls=7", "fs=lots", "fs=1e400"] {
            assert!(!errors(&run(q)).is_empty(), "{q}");
            let f = fix(&cfg(), &parse_query(q));
            assert!(errors(&f.output).is_empty() && !f.changes.is_empty(), "{q}");
        }
    }

    #[test]
    fn scheduled_start_wraps_the_capture_in_an_at_job() {
        let o = run("v=1&if=lagg0&w=1&dd=1&lg=1&bg=1&f.s=600&f.C=500&f.W=5&as=time&asv=01:00");
        assert!(errors(&o).is_empty(), "{:?}", o.issues);
        let lines: Vec<&str> = o.command.lines().collect();
        assert_eq!((lines[0], lines[2], lines.len()), ("at 01:00 <<'EOF'", "EOF", 3));
        // Inside the job the log redirect is the /bin/sh form.
        assert!(lines[1].starts_with("mkdir -p ") && lines[1].contains(".log 2>&1 &") && !lines[1].contains("&>>"));
        assert_eq!(o.stop_command.as_deref(), Some("killall -SIGINT tcpdump"));

        let o = run("v=1&t=linux&w=1&f.C=500&f.W=5&as=date&asv=2026-11-12T01:30");
        assert!(o.command.starts_with("sudo at -t 202611120130 <<'EOF'\n"));
        let o = run("v=1&w=1&f.C=500&f.W=5&as=delay&asv=90&asu=minutes");
        assert!(o.command.starts_with("at now + 90 minutes <<'EOF'\n"));
    }

    #[test]
    fn scheduled_stop_adds_a_second_job() {
        let o = run("v=1&sc=all&if=lagg0&w=1&bg=1&f.C=500&f.W=5&as=time&asv=01:00&ae=time&aev=05:00");
        assert!(o.command.ends_with("EOF\nat 05:00 <<'EOF'\nisi_for_array -X 'killall -SIGINT tcpdump'\nEOF"), "{}", o.command);
        // Starting now: the stop is queued first, then the capture runs.
        let o = run("v=1&if=lagg0&w=1&bg=0&lg=0&f.C=500&f.W=5&ae=delay&aev=2&aeu=hours");
        assert!(o.command.starts_with("at now + 2 hours <<'EOF'\nkillall -SIGINT tcpdump\nEOF\nmkdir -p "), "{}", o.command);
        assert!(o.stop_command.is_some());
        let o = run("v=1&w=1&f.C=500&f.W=5&as=delay&asv=3&asu=hours&ae=delay&aev=60&aeu=minutes");
        assert!(warnings(&o).iter().any(|w| w.contains("at or before the start")));
    }

    #[test]
    fn schedule_values_are_validated_and_fixable() {
        for q in [
            "as=time&asv=25:00", "as=time&asv=1am;reboot", "as=date&asv=2026-13-01T00:00", "as=date&asv=tomorrow",
            "as=delay&asv=0", "as=delay&asv=5&asu=fortnights", "as=delay&asv=5;id", "ae=time&aev=", "ae=delay&aev=x",
        ] {
            let o = run(q);
            assert!(!errors(&o).is_empty() && o.command.is_empty(), "{q}");
            let f = fix(&cfg(), &parse_query(q));
            assert!(errors(&f.output).is_empty() && !f.changes.is_empty(), "{q}");
            assert!(!f.output.command.contains("at "), "{q}");
        }
    }

    #[test]
    fn placeholders_stay_in_the_command_until_filled() {
        let q = "v=1&sc=nodes&n=<nodes>&if=<interface>&w=1&sd=<case number>&bg=1&f.s=600&f.C=500&f.W=5&h=<client IP address>";
        let o = run(q);
        assert!(errors(&o).is_empty(), "{:?}", o.issues);
        assert!(o.command.contains("isi_for_array -n<nodes> 'tcpdump -i <interface> "));
        assert!(o.command.contains("/ifs/data/Isilon_Support/<case number>/$(date +%F)/") && o.command.contains("\"host <client IP address>\""));
        assert_eq!(o.placeholders.len(), 4);
        assert!(warnings(&o).iter().any(|w| w.starts_with("Fill in <interface>, <client IP address>, <nodes>, <case number>")));
        // The settings keep the placeholders so the link can be passed on again.
        assert_eq!(o.state.ifaces, ["<interface>"]);
    }

    #[test]
    fn filled_placeholders_are_substituted_and_checked() {
        let q = "v=1&sc=nodes&n=<nodes>&if=<interface>&w=1&sd=<case number>&bg=1&f.s=600&f.C=500&f.W=5&h=<client IP address>";
        let filled = format!("{q}&ph.<interface>=bxe0, bxe1&ph.<nodes>=1-3&ph.<case number>=SR12345&ph.<client IP address>=10.0.0.5");
        let o = run(&filled);
        assert!(errors(&o).is_empty() && !warnings(&o).iter().any(|w| w.contains("Fill in")), "{:?}", o.issues);
        assert!(o.command.contains("isi_for_array -n1-3 'for iface in bxe0 bxe1; do tcpdump -i ${iface} "));
        assert!(o.command.contains("/Isilon_Support/SR12345/") && o.command.contains(" host 10.0.0.5 ") && !o.command.contains('<'));
        // Two interfaces on three nodes are counted from the filled-in values.
        assert_eq!(o.estimate.as_ref().unwrap().headline, "Up to 15.0 GB");
        assert_eq!((o.state.ifaces[0].as_str(), o.state.fills["<interface>"].as_str()), ("<interface>", "bxe0, bxe1"));

        // A bad value is reported against the placeholder, and never reaches the command.
        for bad in ["ph.<interface>=eth0;reboot", "ph.<client IP address>=$(id)", "ph.<nodes>=1;id", "ph.<case number>=../x", "ph.<interface>=<other>"] {
            let o = run(&format!("{q}&{bad}"));
            assert!(o.issues.iter().any(|i| i.level == "error" && i.field.starts_with("ph.<")), "{bad}");
            assert!(o.command.is_empty(), "{bad}");
            let f = fix(&cfg(), &parse_query(&format!("{q}&{bad}")));
            assert!(errors(&f.output).is_empty() && f.changes.len() == 1, "{bad}: {:?}", f.changes);
            assert!(f.output.command.contains("<interface>") || f.output.command.contains("<client IP address>"));
        }
        // A fill for a placeholder that is no longer in the settings is dropped.
        assert!(run("v=1&if=lagg0&ph.<interface>=bxe0").state.fills.is_empty());
    }

    #[test]
    fn recurring_capture_installs_a_script_and_a_crontab_line() {
        let o = run("v=1&if=lagg0&w=1&dd=1&lg=1&bg=1&f.s=600&f.G=300&f.W=2&as=cron&ck=weekdays&ct=01:30&cd=5,1,1&cn=nightly");
        assert!(errors(&o).is_empty(), "{:?}", o.issues);
        let lines: Vec<&str> = o.command.lines().collect();
        assert_eq!(lines[0], "mkdir -p /ifs/data/Isilon_Support");
        assert_eq!(lines[1], "cat > /ifs/data/Isilon_Support/tcpdump_nightly.sh <<'EOF'");
        assert_eq!(lines[2], "#!/bin/sh");
        assert!(lines[3].contains("tcpdump -i lagg0") && lines[3].contains("%F_%H%M%S") && lines[3].contains(".log 2>&1 &"));
        assert_eq!((lines[4], lines[5]), ("EOF", "chmod +x /ifs/data/Isilon_Support/tcpdump_nightly.sh"));
        assert_eq!(lines[6], "(crontab -l 2>/dev/null | grep -v '/ifs/data/Isilon_Support/tcpdump_nightly.sh'; echo '30 1 * * 1,5 /ifs/data/Isilon_Support/tcpdump_nightly.sh') | crontab -");
        assert_eq!(o.schedule_summary.as_deref(), Some("Runs at 01:30 on Monday, Friday."));
        assert_eq!(o.manage.len(), 2);
        assert!(o.manage[1].command.contains("grep -v '/ifs/data/Isilon_Support/tcpdump_nightly.sh'") && o.manage[1].command.contains("rm -f"));
        // It ends by itself (-G with -W), so the only warning is about files piling up.
        assert_eq!(warnings(&o).len(), 1, "{:?}", o.issues);

        let o = run("v=1&t=linux&w=1&f.C=500&f.W=5&as=cron&ck=hours&ch=6&ae=delay&aev=30&aeu=minutes");
        assert!(o.command.starts_with("sudo tee /usr/local/sbin/tcpdump_capture.sh >/dev/null <<'EOF'\n#!/bin/sh\n(sleep 1800; sudo pkill -INT tcpdump) &\n"));
        assert!(o.command.contains("echo '0 */6 * * * /usr/local/sbin/tcpdump_capture.sh') | sudo crontab -"));
        // A ring buffer never ends: warned unless a stop delay is set.
        let o = run("v=1&w=1&f.C=500&f.W=5&as=cron&ck=daily");
        assert!(warnings(&o).iter().any(|w| w.contains("does not end by itself")));
    }

    #[test]
    fn at_jobs_come_with_list_and_remove_commands() {
        let o = run("v=1&w=1&f.C=500&f.W=5&as=time&asv=01:00");
        let commands: Vec<&str> = o.manage.iter().map(|m| m.command.as_str()).collect();
        assert_eq!(commands, ["atq", "at -c <job number>", "atrm <job number>"]);
        let o = run("v=1&t=linux&w=1&f.C=500&f.W=5&ae=delay&aev=2&aeu=hours");
        assert_eq!(o.manage[0].command, "sudo atq");
        assert!(run("v=1&w=1&f.C=500&f.W=5").manage.is_empty());
    }

    #[test]
    fn cron_values_are_validated_and_fixable() {
        for q in [
            "as=cron&ck=daily&ct=25:00", "as=cron&ck=weekdays&cd=7", "as=cron&ck=weekdays&cd=x&ct=bad", "as=cron&ck=hours&ch=0",
            "as=cron&ck=custom&cx=* * * *", "as=cron&ck=custom&cx=0 1 * * *; reboot", "as=cron&ck=custom&cx=0 1 * * $(id)",
            "as=cron&cn=a/b", "as=cron&cn=x;id", "as=cron&ae=time&aev=05:00",
        ] {
            let o = run(q);
            assert!(!errors(&o).is_empty() && o.command.is_empty(), "{q}");
            let f = fix(&cfg(), &parse_query(q));
            assert!(errors(&f.output).is_empty() && !f.changes.is_empty(), "{q}: {:?}", f.output.issues);
            assert!(!f.output.command.contains("reboot") && !f.output.command.contains("$(id)") && !f.output.command.contains(";id"));
        }
    }

    #[test]
    fn summary_explains_the_command_in_plain_words() {
        let o = run("v=1&if=lagg0&w=1&dd=1&lg=1&bg=1&f.s=600&f.G=3600&f.W=4&h=192.168.1.1");
        assert_eq!(o.summary, [
            "Captures on lagg0 on this node.",
            "Only traffic to or from 192.168.1.1.",
            "Saves the first 600 bytes of each packet to /ifs/data/Isilon_Support/$(date +%F).",
            "Starts a new file every 1 hour and ends by itself after 4 files (4 hours).",
            "Runs in the background, with tcpdump's own messages saved to a .log file beside the capture.",
        ]);

        let o = run("v=1&sc=nodes&n=1,2&if=bxe0,bxe1&sd=NDMP&w=1&dd=1&lg=0&bg=1&f.s=0&f.C=1024&f.W=10&f.B=65536&h=10.0.0.1,10.0.0.2&net=172.26.0.0/16&p=445,2049&pr=tcp&as=time&asv=01:00&ae=delay&aev=1&aeu=hours");
        assert_eq!(o.summary, [
            "Captures on bxe0 and bxe1 on nodes 1,2.",
            "Only traffic to or from 10.0.0.1, 10.0.0.2 or network 172.26.0.0/16, and on ports 445 or 2049, and using tcp.",
            "Saves whole packets to /ifs/data/Isilon_Support/NDMP/$(date +%F).",
            "Keeps the newest 10 files of 1024 MB per interface, overwriting the oldest, until it is stopped.",
            "Runs in the background.",
            "Also: buffer size (-B) set to 65536.",
            "Starts at 01:00 (queued with at), not straight away.",
            "A stop is queued for 1 hour after the command is pasted.",
        ]);

        let o = run("v=1&t=linux&im=all&w=0&f.s=0&f.c=1&f.nn=1&f.v=1&q=ether[12:2]=0x88cc or ether[20:2]=0x2000");
        assert_eq!(o.summary, [
            "Captures on every interface.",
            "Only traffic matching \"ether[12:2]=0x88cc or ether[20:2]=0x2000\".",
            "Prints packets to the screen; nothing is saved.",
            "Stops after 1 packet.",
            "Also: no name lookups (-nn) and verbose (-v).",
        ]);

        let o = run("v=1&sc=all&if=<interface>&w=1&f.C=500&as=cron&ck=weekdays&ct=22:15&cd=1,5&ae=delay&aev=90&aeu=minutes");
        assert_eq!(o.summary[0], "Captures on <interface> on every node.");
        assert_eq!(o.summary[1], "All traffic is captured; there is no filter.");
        assert_eq!(o.summary[3], "Starts a new 500 MB file each time one fills, with no limit on how many.");
        assert_eq!(o.summary[5], "Runs at 22:15 on Monday, Friday. Installed as a cron job that runs a script.");
        assert_eq!(o.summary[6], "Each run is stopped after 1 hour 30 minutes.");

        // No summary while there is no command.
        assert!(run("v=1&f.s=abc").summary.is_empty());
        // A filled-in placeholder is described by its value.
        assert_eq!(run("v=1&if=<interface>&w=0&ph.<interface>=ix0,ix1").summary[0], "Captures on ix0 and ix1 on this node.");
    }

    #[test]
    fn fix_moves_values_to_the_nearest_valid_one() {
        let f = fix(&cfg(), &parse_query("v=1&t=powerscale&f.s=99999999&f.C=0&f.B=abc&f.W=4"));
        assert!(errors(&f.output).is_empty(), "{:?}", f.output.issues);
        let flags = &f.output.state.flags;
        // Too-large snap lengths mean "everything", so they go to 0, not the limit.
        assert_eq!((flags["s"].as_str(), flags["C"].as_str(), flags["B"].as_str()), ("0", "1", "4096"));
        assert_eq!(f.changes.len(), 3, "{:?}", f.changes);
        assert!(f.changes[0].contains("-s changed from 99999999 to 0."));

        // Without over_max a value above the limit moves to the limit; below min moves to min.
        let f = fix(&cfg(), &parse_query("v=1&f.s=-5&f.C=999999999"));
        let flags = &f.output.state.flags;
        assert_eq!((flags["s"].as_str(), flags["C"].as_str()), ("0", "102400"));
    }

    #[test]
    fn fix_switches_off_clashing_and_orphaned_flags() {
        let f = fix(&cfg(), &parse_query("v=1&f.C=500&f.G=60&f.W=5"));
        let flags = &f.output.state.flags;
        assert!(flags.contains_key("C") && flags.contains_key("W") && !flags.contains_key("G"));
        let f = fix(&cfg(), &parse_query("v=1&f.W=5"));
        assert!(!f.output.state.flags.contains_key("W"));
        assert!(f.changes[0].contains("-W switched off"));
        assert!(errors(&f.output).is_empty());
    }

    #[test]
    fn fix_clears_everything_the_injection_test_rejects() {
        for q in [
            "if=eth0;reboot,lagg0", "if=$(reboot)", "sd=../../etc", "sc=nodes&n=1;reboot", "h=1.1.1.1;reboot,10.0.0.1",
            "p=53;reboot,445", "net=10.0.0.0/8`id`", "q=port 53; reboot", "q=!tcp", "pr=tcp;id", "v=1&f.s=1;id",
            "v=1&t=linux&f.Z=root;id", "t=nope",
        ] {
            let f = fix(&cfg(), &parse_query(q));
            assert!(errors(&f.output).is_empty(), "{q}: {:?}", f.output.issues);
            assert!(!f.changes.is_empty(), "{q}");
            assert!(!f.output.command.is_empty() && !f.output.command.contains("reboot") && !f.output.command.contains(";id"), "{q}");
        }
        let f = fix(&cfg(), &parse_query("if=eth0;reboot,lagg0&h=1.1.1.1;reboot,10.0.0.1"));
        assert_eq!(f.output.state.ifaces, ["lagg0"]);
        assert_eq!(f.output.state.hosts, ["10.0.0.1"]);
    }

    #[test]
    fn fix_leaves_a_valid_setup_alone() {
        let q = "v=1&t=powerscale&if=lagg0&w=1&dd=1&lg=1&bg=1&f.s=600&f.G=3600&f.W=4&h=192.168.1.1";
        let f = fix(&cfg(), &parse_query(q));
        assert!(f.changes.is_empty());
        assert_eq!(f.output.command, run(q).command);
    }

    #[test]
    fn raw_filter_with_shell_characters_is_quoted() {
        let o = run("v=1&t=linux&w=0&q=tcp[13] %26 2 != 0");
        assert_eq!(o.command, "sudo tcpdump -i eth0 \"tcp[13] & 2 != 0\"");
    }

    #[test]
    fn query_decoding() {
        assert_eq!(parse_query("?a=b+c&d=%28x%29&e"), vec![
            ("a".to_string(), "b c".to_string()),
            ("d".to_string(), "(x)".to_string()),
            ("e".to_string(), String::new()),
        ]);
        assert_eq!(parse_query("a=100%"), vec![("a".to_string(), "100%".to_string())]);
    }

    #[test]
    fn render_does_not_rescan_values() {
        assert_eq!(render("{a}-{b}-{c}", &[("a", "{b}"), ("b", "x")]), "{b}-x-{c}");
    }
}
