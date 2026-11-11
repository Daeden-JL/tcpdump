//! Turns a URL query string into builder state and a shell command.
//!
//! The generated command is meant to be pasted into a root shell, and the
//! query string can come from a link someone else sent. Every user-supplied
//! value is therefore checked against a conservative character set, and no
//! command is produced at all while any error is outstanding.

use crate::config::{Config, Flag, Target, ValueType};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Serialize)]
pub struct Issue {
    pub level: &'static str,
    pub field: String,
    pub message: String,
}

/// The resolved form state, sent back so the page can fill itself in.
#[derive(Debug, Serialize)]
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
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub command: String,
    pub stop_command: Option<String>,
    pub issues: Vec<Issue>,
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

fn split_list(s: &str) -> Vec<String> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

fn all_chars(s: &str, extra: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

fn valid_port(p: &str) -> bool {
    let ok = |x: &str| x.parse::<u16>().is_ok();
    match p.split_once('-') {
        Some((a, b)) => ok(a) && ok(b),
        None => ok(p),
    }
}

fn valid_iface(s: &str) -> bool {
    all_chars(s, "_.:-")
}

fn valid_host(s: &str) -> bool {
    all_chars(s, ".:-")
}

fn valid_net(s: &str) -> bool {
    all_chars(s, ".:/") && s.starts_with(|c: char| c.is_ascii_hexdigit())
}

fn valid_subdir(s: &str) -> bool {
    all_chars(s, "_./-") && !s.split('/').any(|p| p == ".." || p.is_empty())
}

fn valid_nodes(s: &str) -> bool {
    all_chars(s, ",-") && s.starts_with(|c: char| c.is_ascii_digit())
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

    State {
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
    }
}

fn target_of<'a>(cfg: &'a Config, st: &State) -> &'a Target {
    cfg.targets.iter().find(|t| t.id == st.target).unwrap_or(&cfg.targets[0])
}

/// Validate the state and write the command.
fn write_command(cfg: &Config, st: State, mut cx: Ctx) -> Output {
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
        cmd.push_str(&render(&target.log_redirect, &[("log", &log), ("dir", &dir)]));
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
    let stop_command = background.then(|| wrap(&target.stop, &mut cx));

    let failed = cx.issues.iter().any(|i| i.level == "error");
    Output {
        command: if failed { String::new() } else { cmd },
        stop_command: if failed { None } else { stop_command },
        issues: cx.issues,
        state: st,
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
