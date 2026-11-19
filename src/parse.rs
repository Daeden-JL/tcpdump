//! Reads a hand-edited command back into builder state.
//!
//! This is deliberately forgiving: it recognises what the builder has a
//! setting for, and reports everything else as a note instead of failing.
//! The result is a query (the same form as a share link), so it goes through
//! the builder's normal validation before any command is shown again.

use crate::builder::{is_placeholder, split_atoms, valid_subdir, AT_UNITS};
use crate::config::{Cluster, Config, Flag, Target, ValueType};

pub struct Parsed {
    pub pairs: Vec<(String, String)>,
    pub notes: Vec<String>,
    /// The `-w` path as it was written in the text, if there was one.
    pub out_path: Option<String>,
}

/// tcpdump options that consume the following word, used to skip the value
/// of an option the config does not list.
const TAKES_VALUE: &[&str] = &[
    "-B", "-c", "-C", "-E", "-F", "-G", "-i", "-j", "-m", "-M", "-Q", "-r", "-s", "-T", "-V", "-w", "-W", "-y",
    "-z", "-Z",
];

struct Scan {
    /// Words of one simple command, with whether each was double-quoted.
    tokens: Vec<(String, bool)>,
    /// Byte offset where the command ended (a redirect, `&`, `;`, `'`...).
    stop: usize,
}

/// Split shell text into words up to the end of the first simple command.
/// `$(...)` and backtick substitutions stay inside their word.
///
/// `wrapped` says the text sits inside a single-quoted wrapper such as
/// `isi_for_array '...'`, where the next `'` ends the command. Otherwise a
/// `'` opens a quoted word, as in `tcpdump -i ix1 '(port 53)'`.
fn scan(s: &str, wrapped: bool) -> Scan {
    let mut tokens = Vec::new();
    let (mut cur, mut has, mut quoted) = (String::new(), false, false);
    let (mut in_quotes, mut in_single, mut in_ticks, mut depth) = (false, false, false, 0usize);
    let mut stop = s.len();
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if in_single {
            if c == '\'' {
                in_single = false;
            } else {
                cur.push(c);
            }
        } else if in_ticks {
            cur.push(c);
            in_ticks = c != '`';
        } else if depth > 0 {
            cur.push(c);
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
        } else if c == '$' && it.peek().is_some_and(|p| p.1 == '(') {
            it.next();
            cur.push_str("$(");
            (depth, has) = (1, true);
        } else if c == '`' {
            cur.push(c);
            (in_ticks, has) = (true, true);
        } else if in_quotes {
            if c == '"' {
                in_quotes = false;
            } else {
                cur.push(c);
            }
        } else {
            match c {
                '"' => (in_quotes, quoted, has) = (true, true, true),
                '\\' => {
                    if let Some((_, n)) = it.next() {
                        cur.push(n);
                        has = true;
                    }
                }
                // A placeholder such as <client IP address> stays one word.
                '<' if s[i..].find('>').is_some_and(|end| !s[i..i + end].contains([';', '&', '\''])) => {
                    cur.push(c);
                    has = true;
                    for (_, n) in it.by_ref() {
                        cur.push(n);
                        if n == '>' {
                            break;
                        }
                    }
                }
                '\'' if !wrapped => (in_single, quoted, has) = (true, true, true),
                ';' | '&' | '>' | '<' | '|' | '\'' => {
                    stop = i;
                    break;
                }
                c if c.is_whitespace() => {
                    if has {
                        tokens.push((std::mem::take(&mut cur), quoted));
                        (has, quoted) = (false, false);
                    }
                }
                _ => {
                    cur.push(c);
                    has = true;
                }
            }
        }
    }
    if has {
        tokens.push((cur, quoted));
    }
    // `2>&1` leaves its file descriptor behind as a word.
    if s[stop..].starts_with('>') && tokens.last().is_some_and(|t| t.0 == "1" || t.0 == "2") {
        tokens.pop();
    }
    Scan { tokens, stop }
}

/// Offset just past the `tcpdump` word that starts the capture command.
fn find_tcpdump(text: &str) -> Option<usize> {
    text.match_indices("tcpdump").map(|(i, m)| (i, i + m.len())).find_map(|(i, end)| {
        let before = text[..i].chars().next_back();
        let after = text[end..].chars().next();
        let starts_word = before.is_none_or(|c| !(c.is_alphanumeric() || "_-.".contains(c)));
        (starts_word && after.is_some_and(char::is_whitespace)).then_some(end)
    })
}

fn pick_target<'a>(cfg: &'a Config, text: &str, hint: &str) -> &'a Target {
    let score = |t: &Target| {
        let cluster_word = t.cluster.as_ref().and_then(|c| c.all_nodes.split_whitespace().next());
        let mut s = 0;
        s += 4 * usize::from(cluster_word.is_some_and(|w| text.contains(w)));
        s += 2 * usize::from(!t.output_dir.is_empty() && text.contains(&t.output_dir));
        s += 2 * usize::from(t.command.contains(' ') && text.contains(&t.command));
        s + usize::from(t.id == hint)
    };
    let mut best = &cfg.targets[0];
    for t in &cfg.targets[1..] {
        if score(t) > score(best) {
            best = t;
        }
    }
    best
}

/// `(scope, nodes)` from a cluster wrapper such as `isi_for_array -n1,2 '...'`.
fn cluster_scope(text: &str, cluster: &Cluster) -> Option<(&'static str, String)> {
    let word = cluster.all_nodes.split_whitespace().next()?;
    let after = &text[text.find(word)? + word.len()..];
    let nodes_flag = cluster
        .selected_nodes
        .split("{nodes}")
        .next()
        .and_then(|p| p.split_whitespace().last())
        .filter(|f| f.starts_with('-'));
    let mut words = after.split_whitespace();
    while let Some(w) = words.next() {
        if w.starts_with(['\'', '"']) {
            break;
        }
        if let Some(value) = nodes_flag.and_then(|f| w.strip_prefix(f)) {
            let value = if value.is_empty() { words.next().unwrap_or("") } else { value };
            return Some(("nodes", value.to_string()));
        }
    }
    Some(("all", String::new()))
}

/// A run of short options written together, taken apart.
struct ShortRun<'a> {
    /// Keys of the Bool flags in it.
    bools: Vec<&'a str>,
    /// Options the builder has no setting for (`-X`).
    unknown: Vec<String>,
    /// The option that takes a value, if the run ends in one, with whatever
    /// was attached to it (`-s0` gives `("-s", "0")`).
    tail: Option<(String, String)>,
}

/// Split a run of short options written together, as tcpdump accepts them:
/// `nnv` is `-nn -v`, `ni` is `-n -i` with the value in the next word, and
/// `nvs0` is `-n -v -s 0`.
fn split_cluster<'a>(mut rest: &str, flags: &[&'a Flag]) -> ShortRun<'a> {
    let mut keys: Vec<&str> = flags
        .iter()
        .filter(|f| f.value_type == ValueType::Bool && !f.name.starts_with("--"))
        .map(|f| f.key())
        .collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    let mut out = ShortRun { bools: Vec::new(), unknown: Vec::new(), tail: None };
    while let Some(first) = rest.chars().next() {
        if let Some(key) = keys.iter().find(|k| rest.starts_with(**k)) {
            out.bools.push(*key);
            rest = &rest[key.len()..];
            continue;
        }
        let name = format!("-{first}");
        let after = &rest[first.len_utf8()..];
        let known_value = name == "-i" || name == "-w" || flags.iter().any(|f| f.name == name && f.value_type != ValueType::Bool);
        if known_value || TAKES_VALUE.contains(&name.as_str()) {
            // Everything after a value-taking option is its value.
            out.tail = Some((name, after.to_string()));
            break;
        }
        out.unknown.push(name);
        rest = after;
    }
    out
}

#[derive(Default)]
struct Guided {
    hosts: Vec<String>,
    nets: Vec<String>,
    ports: Vec<String>,
    proto: String,
}

/// Fit a filter into the guided fields when it has the shape the builder
/// writes: groups of hosts/networks, ports and one protocol joined by `and`.
fn guided_filter(words: &[String], protocols: &[String]) -> Option<Guided> {
    let spaced = words.join(" ").replace('(', " ( ").replace(')', " ) ");
    let atoms = split_atoms(&spaced, false);
    let toks: Vec<&str> = atoms.iter().map(String::as_str).collect();
    let mut parts: Vec<&[&str]> = Vec::new();
    let (mut depth, mut from) = (0i32, 0);
    for (i, t) in toks.iter().enumerate() {
        match *t {
            "(" => depth += 1,
            ")" => depth -= 1,
            "and" if depth == 0 => {
                parts.push(&toks[from..i]);
                from = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&toks[from..]);

    let mut g = Guided::default();
    let (mut seen_addr, mut seen_port) = (false, false);
    for part in parts {
        let part = match part {
            ["(", inner @ .., ")"] => inner,
            other => other,
        };
        if part.is_empty() || part.iter().any(|t| *t == "(" || *t == ")") {
            return None;
        }
        let terms: Vec<&[&str]> = part.split(|t| *t == "or").collect();
        let is_addr = |t: &&[&str]| matches!(t, ["host" | "net", _]);
        let is_port = |t: &&[&str]| matches!(t, ["port" | "portrange", _]);
        if terms.iter().all(is_addr) && !seen_addr {
            seen_addr = true;
            for t in terms {
                if t[0] == "host" { g.hosts.push(t[1].into()) } else { g.nets.push(t[1].into()) }
            }
        } else if terms.iter().all(is_port) && !seen_port {
            seen_port = true;
            g.ports.extend(terms.iter().map(|t| t[1].to_string()));
        } else if let [[p]] = terms[..] {
            if !g.proto.is_empty() || !protocols.iter().any(|x| x == p) {
                return None;
            }
            g.proto = p.to_string();
        } else {
            return None;
        }
    }
    Some(g)
}

/// Turn an `at` time argument back into (mode, value, unit).
fn read_at_spec(spec: &str) -> Option<(&'static str, String, String)> {
    let words: Vec<&str> = spec.split_whitespace().collect();
    match words[..] {
        ["now", "+", n, unit] => {
            let unit = AT_UNITS.iter().find(|u| **u == unit || u.trim_end_matches('s') == unit)?;
            Some(("delay", n.to_string(), unit.to_string()))
        }
        ["-t", t] if t.len() == 12 && t.bytes().all(|c| c.is_ascii_digit()) => {
            Some(("date", format!("{}-{}-{}T{}:{}", &t[..4], &t[4..6], &t[6..8], &t[8..10], &t[10..]), String::new()))
        }
        [t] if t.len() == 5 && t.as_bytes()[2] == b':' => Some(("time", t.to_string(), String::new())),
        _ => None,
    }
}

/// Map a cron expression back onto the repeat settings.
fn read_cron(expr: &str) -> Vec<(String, String)> {
    let f: Vec<&str> = expr.split_whitespace().collect();
    let num = |s: &str| s.parse::<u32>().ok();
    let pair = |k: &str, v: String| (k.to_string(), v);
    let mut out = vec![pair("as", "cron".into())];
    match f[..] {
        [m, h, "*", "*", dow] if num(m).is_some() && num(h).is_some() && (dow == "*" || dow.split(',').all(|d| num(d).is_some())) => {
            out.push(pair("ct", format!("{:02}:{:02}", num(h).unwrap_or(0), num(m).unwrap_or(0))));
            if dow == "*" {
                out.push(pair("ck", "daily".into()));
            } else {
                out.push(pair("ck", "weekdays".into()));
                out.push(pair("cd", dow.to_string()));
            }
        }
        ["0", h, "*", "*", "*"] if h.strip_prefix("*/").and_then(num).is_some() => {
            out.push(pair("ck", "hours".into()));
            out.push(pair("ch", h[2..].to_string()));
        }
        _ => {
            out.push(pair("ck", "custom".into()));
            out.push(pair("cx", f.join(" ")));
        }
    }
    out
}

/// Pull scheduling out of the text: `at ... <<'EOF'` jobs, and the script
/// plus crontab line of a recurring capture. Returns the text with only the
/// capture command left in it, and the schedule as query pairs.
fn split_schedule(text: &str, notes: &mut Vec<String>) -> (String, Vec<(String, String)>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut put = |k: &str, v: &str| pairs.push((k.to_string(), v.to_string()));
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        let plain = line.strip_prefix("sudo ").unwrap_or(line);
        i += 1;
        // The crontab line of a recurring capture: ... echo '<expr> <script>' ...
        if plain.contains("crontab") {
            let entry = plain.split_once("echo '").and_then(|(_, rest)| rest.split_once('\''));
            if let Some((entry, _)) = entry {
                let fields: Vec<&str> = entry.split_whitespace().collect();
                if fields.len() >= 6 {
                    for (k, v) in read_cron(&fields[..5].join(" ")) {
                        put(&k, &v);
                    }
                    let name = fields[5].rsplit('/').next().and_then(|f| f.strip_prefix("tcpdump_")).and_then(|f| f.strip_suffix(".sh"));
                    if let Some(name) = name {
                        put("cn", name);
                    }
                }
            }
            continue;
        }
        if plain.starts_with("chmod ") {
            continue;
        }
        // An `at` job queued from a script file: at -f <script> <when>
        if let Some(rest) = plain.strip_prefix("at -f ") {
            let (path, spec) = rest.trim().split_once(' ').unwrap_or((rest.trim(), ""));
            match read_at_spec(spec) {
                Some((mode, value, unit)) => {
                    put("as", mode);
                    put("asv", &value);
                    if !unit.is_empty() {
                        put("asu", &unit);
                    }
                    put("asc", "1");
                    let name = path.rsplit('/').next().and_then(|f| f.strip_prefix("tcpdump_")).and_then(|f| f.strip_suffix(".sh"));
                    if let Some(name) = name {
                        put("cn", name);
                    }
                }
                None => notes.push(format!("The schedule \"at {}\" is not a form the builder can set and was left out.", spec.trim())),
            }
            continue;
        }
        let Some((head, delimiter)) = plain.split_once("<<") else {
            kept.push(lines[i - 1]);
            continue;
        };
        // A here-document: collect its body up to the delimiter.
        let delimiter = delimiter.trim().trim_start_matches('-').trim_matches(['\'', '"']);
        let start = i;
        while i < lines.len() && lines[i].trim() != delimiter {
            i += 1;
        }
        let body = &lines[start..i];
        i += 1;
        match head.strip_prefix("at ") {
            Some(spec) => {
                // A job with a capture in it is the start; anything else is the stop.
                let is_start = find_tcpdump(&body.join(" ")).is_some();
                match read_at_spec(spec) {
                    Some((mode, value, unit)) => {
                        let (m, v, u) = if is_start { ("as", "asv", "asu") } else { ("ae", "aev", "aeu") };
                        put(m, mode);
                        put(v, &value);
                        if !unit.is_empty() {
                            put(u, &unit);
                        }
                    }
                    None => notes.push(format!("The schedule \"at {}\" is not a form the builder can set and was left out.", spec.trim())),
                }
                if is_start {
                    kept.extend(body);
                }
            }
            // The script of a recurring capture (cat > file, or tee file).
            None => {
                for line in body {
                    let t = line.trim();
                    let sleep = t.strip_prefix("(sleep ").and_then(|r| r.split_once(';')).and_then(|(n, _)| n.trim().parse::<u64>().ok());
                    if let Some(seconds) = sleep {
                        let (n, unit) = match seconds {
                            s if s % 86400 == 0 => (s / 86400, "days"),
                            s if s % 3600 == 0 => (s / 3600, "hours"),
                            s => (s / 60, "minutes"),
                        };
                        put("ae", "delay");
                        put("aev", &n.to_string());
                        put("aeu", unit);
                    } else if !t.starts_with("#!") {
                        kept.push(line);
                    }
                }
            }
        }
    }
    (kept.join("\n"), pairs)
}

fn simple_name(s: &str, extra: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

/// Parse `text`; `hint` is the target currently selected in the builder.
/// Returns `None` when there is no tcpdump command in the text at all.
pub fn parse(cfg: &Config, text: &str, hint: &str) -> Option<Parsed> {
    let target = pick_target(cfg, text, hint);
    let mut notes: Vec<String> = Vec::new();
    let (text, schedule) = split_schedule(text, &mut notes);
    let text = text.replace("\\\n", " ").replace(['\n', '\r'], " ");
    let start = find_tcpdump(&text)?;
    let wrapped = text[..start].matches('\'').count() % 2 == 1;
    let Scan { tokens, stop } = scan(&text[start..], wrapped);
    let tail = &text[start + stop..];

    let mut pairs: Vec<(String, String)> = vec![("v".into(), "1".into()), ("t".into(), target.id.clone())];
    let mut put = |k: &str, v: &str| pairs.push((k.to_string(), v.to_string()));

    if let Some((scope, nodes)) = target.cluster.as_ref().and_then(|c| cluster_scope(&text, c)) {
        put("sc", scope);
        if scope == "nodes" {
            put("n", &nodes);
        }
    } else {
        put("sc", "node");
    }

    // Options, up to the first word that is not one: that starts the filter.
    let flags: Vec<&Flag> = cfg.flags.iter().filter(|f| f.applies_to(&target.id)).collect();
    let (mut iface, mut out_path): (Option<String>, Option<String>) = (None, None);
    let mut set_flags: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let (tok, quoted) = &tokens[i];
        if *quoted || !tok.starts_with('-') || tok == "-" {
            break;
        }
        let next = tokens.get(i + 1).map(|t| t.0.clone());
        i += 1;
        let mut take_next = |what: &str, notes: &mut Vec<String>| {
            if next.is_some() {
                i += 1;
            } else {
                notes.push(format!("{what} has no value and was left out."));
            }
            next.clone()
        };
        if let Some(f) = flags.iter().find(|f| f.name == *tok) {
            if f.value_type == ValueType::Bool {
                set_flags.push((f.key().into(), "1".into()));
            } else if let Some(v) = take_next(tok, &mut notes) {
                set_flags.push((f.key().into(), v));
            }
        } else if tok == "-i" {
            iface = take_next(tok, &mut notes);
        } else if tok == "-w" {
            out_path = take_next(tok, &mut notes);
        } else if let Some(run) = tok.strip_prefix('-').filter(|r| !r.starts_with('-')) {
            // Short options, possibly run together: -nnv, -ni <iface>, -s0, -nvs0 ...
            let cluster = split_cluster(run, &flags);
            let left_out = |name: &str| {
                let whole = if name == tok { String::new() } else { format!(" (in {tok})") };
                if cfg.flags.iter().any(|f| f.name == name) {
                    format!("{name}{whole} is not available for {} and was left out.", target.name)
                } else {
                    format!("{name}{whole} is not an option the builder knows and was left out.")
                }
            };
            set_flags.extend(cluster.bools.into_iter().map(|k| (k.to_string(), "1".to_string())));
            notes.extend(cluster.unknown.iter().map(|name| left_out(name)));
            if let Some((name, attached)) = cluster.tail {
                let known = flags.iter().find(|f| f.name == name);
                if name != "-i" && name != "-w" && known.is_none() {
                    // An option the builder lacks: skip its value too.
                    notes.push(left_out(&name));
                    if attached.is_empty() && next.is_some() {
                        i += 1;
                    }
                } else {
                    let value = if attached.is_empty() { take_next(&name, &mut notes) } else { Some(attached) };
                    match (name.as_str(), value, known) {
                        (_, None, _) => {}
                        ("-i", v, _) => iface = v,
                        ("-w", v, _) => out_path = v,
                        (_, Some(v), Some(f)) => set_flags.push((f.key().into(), v)),
                        _ => {}
                    }
                }
            }
        } else {
            notes.push(format!("{tok} is not an option the builder knows and was left out."));
        }
    }
    let filter_words: Vec<String> = tokens[i..].iter().flat_map(|t| split_atoms(&t.0, false)).collect();

    // Interface: a name, the catch-all pseudo-interface, or a loop variable.
    let can_all = target.all_interfaces.is_some() || target.any_interface.is_some();
    let mut names: Vec<String> = Vec::new();
    let mut iface_exclude: Option<String> = None;
    let mut all = false;
    match iface {
        None => notes.push(format!("No -i option found; the interface was set to {}.", target.default_interface)),
        Some(v) if v.starts_with('$') => {
            let var = v.trim_matches(['$', '{', '}']);
            match text.split_once(&format!("for {var} in ")) {
                None => notes.push(format!("The loop that sets {v} was not found; the interface was set to {}.", target.default_interface)),
                Some((_, rest)) => {
                    let end = [rest.find(';'), rest.find(" do ")].into_iter().flatten().min().unwrap_or(rest.len());
                    let list = rest[..end].trim();
                    if list.starts_with('`') || list.starts_with("$(") {
                        all = true;
                        // The builder writes the configured list. Read back the names it
                        // leaves out when only those differ; otherwise say the list was replaced.
                        let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
                        if let Some(configured) = target.all_interfaces.as_ref().filter(|_| target.any_interface.is_none()) {
                            let typed = squash(list);
                            let exclude = configured.split_once("{exclude}").and_then(|(before, after)| {
                                typed.strip_prefix(squash(before).as_str())?.strip_suffix(squash(after).as_str()).map(str::to_string)
                            });
                            match exclude {
                                Some(pattern) => iface_exclude = Some(pattern),
                                None if typed != squash(configured) => notes.push(format!(
                                    "The command lists interfaces with {list} but the builder always uses its configured list, {}. Change all_interfaces in the config to use a different one.",
                                    configured.replace("{exclude}", &target.all_interfaces_exclude)
                                )),
                                None => {}
                            }
                        }
                    } else {
                        names = list
                            .trim_matches(['{', '}'])
                            .split([',', ' '])
                            .filter(|n| !n.is_empty())
                            .map(str::to_string)
                            .collect();
                    }
                }
            }
        }
        Some(v) if target.any_interface.as_deref() == Some(v.as_str()) => all = true,
        Some(v) => names.push(v),
    }
    if all && !can_all {
        notes.push(format!("{} has no \"all interfaces\" setting; the interface was set to {}.", target.name, target.default_interface));
        all = false;
    }
    names.retain(|n| {
        let ok = simple_name(n, "_.:-") || is_placeholder(n);
        if !ok {
            notes.push(format!("{n} is not a real interface name and was left out."));
        }
        ok
    });
    put("im", if all { "all" } else { "named" });
    if let Some(pattern) = iface_exclude.filter(|_| all) {
        put("ix", &pattern);
    }
    if !all && !names.is_empty() {
        put("if", &names.join(","));
    }

    // Output file: only the folder is read back; the file name is generated.
    let mut dated = target.dated_default;
    if let Some(path) = &out_path {
        let dir = path.rsplit_once('/').map_or("", |p| p.0);
        let ds = target.date_subdir.as_str();
        match dir.strip_prefix(target.output_dir.as_str()) {
            Some(rest) => {
                let mut rest = rest.trim_matches('/');
                dated = !ds.is_empty() && (rest == ds || rest.ends_with(&format!("/{ds}")));
                if dated {
                    rest = rest[..rest.len() - ds.len()].trim_matches('/');
                }
                if valid_subdir(rest) {
                    put("sd", rest);
                } else if !rest.is_empty() {
                    notes.push(format!("The folder \"{rest}\" cannot be set in the builder; the standard folder is used."));
                }
            }
            None => {
                dated = !ds.is_empty() && dir.contains(ds);
                notes.push(format!("The output folder is not under {}; the standard folder is used.", target.output_dir));
            }
        }
    }
    put("w", if out_path.is_some() { "1" } else { "0" });
    put("dd", if dated { "1" } else { "0" });

    // What follows the command: a redirect means a log, a lone & means background.
    let logged = tail.starts_with('>') || tail.starts_with("&>");
    let end = [tail.find(';'), tail.find('\''), tail.find(" done")].into_iter().flatten().min().unwrap_or(tail.len());
    let rest = ["&>>", "&>", "2>&1", ">&", "&&"].iter().fold(tail[..end].to_string(), |s, x| s.replace(x, " "));
    put("lg", if logged { "1" } else { "0" });
    put("bg", if rest.contains('&') { "1" } else { "0" });

    for (k, v) in &set_flags {
        if !pairs.iter().any(|(pk, _)| pk == &format!("f.{k}")) {
            pairs.push((format!("f.{k}"), v.clone()));
        }
    }
    let mut put = |k: &str, v: &str| pairs.push((k.to_string(), v.to_string()));

    if !filter_words.is_empty() {
        let filter = filter_words.join(" ");
        if let Some(g) = guided_filter(&filter_words, &cfg.filter.protocols) {
            for (k, v) in [("h", g.hosts.join(",")), ("net", g.nets.join(",")), ("p", g.ports.join(",")), ("pr", g.proto)] {
                if !v.is_empty() {
                    put(k, &v);
                }
            }
        } else {
            put("q", &filter);
        }
    }
    pairs.extend(schedule);
    Some(Parsed { pairs, notes, out_path })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{build, parse_query};

    fn cfg() -> Config {
        Config::parse(include_str!("../tcpdump.toml")).expect("shipped config is valid")
    }

    fn get<'a>(p: &'a Parsed, key: &str) -> Option<&'a str> {
        p.pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn every_example_survives_a_round_trip() {
        let cfg = cfg();
        for ex in &cfg.examples {
            let original = build(&cfg, &parse_query(&ex.query));
            let parsed = parse(&cfg, &original.command, "").expect("command found");
            assert!(parsed.notes.is_empty(), "{}: {:?}", ex.title, parsed.notes);
            let again = build(&cfg, &parsed.pairs);
            assert_eq!(again.command, original.command, "{}", ex.title);
            assert_eq!(again.stop_command, original.stop_command, "{}", ex.title);
        }
    }

    #[test]
    fn scheduled_commands_survive_a_round_trip() {
        let cfg = cfg();
        for q in [
            "v=1&if=lagg0&w=1&dd=1&lg=1&bg=1&f.s=600&f.C=500&f.W=5&as=time&asv=01:00",
            "v=1&sc=all&im=all&w=1&dd=1&lg=1&bg=1&f.s=600&f.C=500&f.W=5&as=date&asv=2026-11-12T01:30&ae=date&aev=2026-11-12T05:00",
            "v=1&if=lagg0&w=1&dd=1&lg=0&bg=0&f.s=600&f.C=500&f.W=5&ae=delay&aev=2&aeu=hours",
            "v=1&t=linux&if=eth0&w=1&dd=1&lg=1&bg=0&f.s=600&f.G=300&f.W=2&as=delay&asv=90&asu=minutes&p=53",
            "v=1&sc=all&if=lagg0&w=1&dd=1&lg=1&bg=1&f.s=600&f.C=500&f.W=5&as=date&asv=2026-11-12T01:30&asc=1&cn=case42&ae=date&aev=2026-11-12T05:00",
            "v=1&t=linux&if=eth0&w=1&dd=1&lg=1&bg=0&f.s=600&f.G=300&f.W=2&as=delay&asv=2&asu=hours&asc=1",
        ] {
            let original = build(&cfg, &parse_query(q));
            assert!(original.command.contains("<<'EOF'"), "{q}");
            let parsed = parse(&cfg, &original.command, "").expect("command found");
            assert!(parsed.notes.is_empty(), "{q}: {:?}", parsed.notes);
            assert_eq!(build(&cfg, &parsed.pairs).command, original.command, "{q}");
        }
        let p = parse(&cfg, "at 1am tomorrow <<EOF\ntcpdump -i eth0 -w /tmp/x.pcap\nEOF", "linux").unwrap();
        assert!(get(&p, "as").is_none() && p.notes.iter().any(|n| n.contains("1am tomorrow")));
    }

    #[test]
    fn hand_written_cluster_loop() {
        let p = parse(
            &cfg(),
            "mkdir -p /ifs/data/Isilon_Support/NDMP/$(date +%F); isi_for_array -n1,2 'for iface in {cxgb0,cxgb1}; do \
             tcpdump -s 900 -G 3600 -W 4 -i $iface -w /ifs/data/Isilon_Support/NDMP/$(date +%F)/$(uname -n).%F_%H%M%S.$iface.pcap \
             host 192.168.1.1 or host 192.168.1.2 &>> /ifs/data/Isilon_Support/NDMP/$(date +%F)/$(uname -n).$(date +%F_%H%M%S).$iface.log & ; done' &",
            "linux",
        )
        .unwrap();
        assert!(p.notes.is_empty(), "{:?}", p.notes);
        let expect = [
            ("t", "powerscale"), ("sc", "nodes"), ("n", "1,2"), ("im", "named"), ("if", "cxgb0,cxgb1"),
            ("w", "1"), ("sd", "NDMP"), ("dd", "1"), ("lg", "1"), ("bg", "1"),
            ("f.s", "900"), ("f.G", "3600"), ("f.W", "4"), ("h", "192.168.1.1,192.168.1.2"),
        ];
        for (k, v) in expect {
            assert_eq!(get(&p, k), Some(v), "{k}");
        }
        assert!(build(&cfg(), &p.pairs).issues.is_empty());
    }

    #[test]
    fn attached_values_custom_folder_and_all_interface_loop() {
        let p = parse(
            &cfg(),
            "isi_for_array -n17 'tcpdump -i lagg0 -s0 -W 10 -C 500 -B 2147483647 -w /ifs/data/Isilon_Support/$(date +%d-%m-%Y)/pcaps/$(uname -n).$(date +%d%m%Y_%H%M%S).pcap net 172.26.0.0/16' &",
            "",
        )
        .unwrap();
        assert_eq!(get(&p, "f.s"), Some("0"));
        assert_eq!(get(&p, "f.B"), Some("2147483647"));
        assert_eq!(get(&p, "net"), Some("172.26.0.0/16"));
        assert_eq!((get(&p, "dd"), get(&p, "bg"), get(&p, "lg")), (Some("0"), Some("0"), Some("0")));
        assert_eq!(p.notes.len(), 1, "{:?}", p.notes);

        let p = parse(
            &cfg(),
            "isi_for_array -X 'for iface in `ifconfig | grep -B2 ether | grep flags | cut -d: -f1` ; do tcpdump -i $iface -s 600 -w /ifs/data/Isilon_Support/$(date +%F)/`hostname`.${iface}_$(date +%F_%H%M%S).pcap -C 500 -W 5 &>> /ifs/x.log & ; done'",
            "",
        )
        .unwrap();
        assert_eq!((get(&p, "sc"), get(&p, "im"), get(&p, "if")), (Some("all"), Some("all"), None));
        assert_eq!((get(&p, "f.C"), get(&p, "f.W")), (Some("500"), Some("5")));
        // The older -B2 list is not what the builder writes, and Parse says so
        // instead of swapping it silently.
        assert_eq!(p.notes.len(), 1, "{:?}", p.notes);
        assert!(p.notes[0].contains("grep -B2 ether") && p.notes[0].contains("grep -B3 ether"));
        let rebuilt = build(&cfg(), &p.pairs).command;
        assert!(rebuilt.contains(r#"for iface in `ifconfig | grep -B3 ether | grep flags | egrep -v "laggport|lo0|ISIINTERNAL" | cut -d: -f1`; do"#), "{rebuilt}");
        // A list that differs only in the names it leaves out is kept, with no note.
        let other = rebuilt.replace("laggport|lo0|ISIINTERNAL", "lo0|vlan");
        let p2 = parse(&cfg(), &other, "").unwrap();
        assert!(p2.notes.is_empty(), "{:?}", p2.notes);
        assert_eq!(get(&p2, "ix"), Some("lo0|vlan"));
        assert_eq!(build(&cfg(), &p2.pairs).command, other);
        // The configured list, with different spacing, parses back with no note.
        let same = rebuilt.replace("| grep flags |", "|  grep flags  |");
        assert!(parse(&cfg(), &same, "").unwrap().notes.is_empty());
    }

    #[test]
    fn linux_live_capture_with_unknown_option_and_raw_filter() {
        let p = parse(&cfg(), "sudo tcpdump -i any -nnv -X -c 5 \"tcp[13] & 2 != 0\"", "powerscale").unwrap();
        assert_eq!((get(&p, "t"), get(&p, "im"), get(&p, "w")), (Some("linux"), Some("all"), Some("0")));
        assert_eq!((get(&p, "f.nn"), get(&p, "f.v"), get(&p, "f.c")), (Some("1"), Some("1"), Some("5")));
        assert_eq!(get(&p, "q"), Some("tcp[13] & 2 != 0"));
        assert_eq!(p.notes.len(), 1);
        assert!(p.notes[0].contains("-X"));
        assert_eq!(build(&cfg(), &p.pairs).command, "sudo tcpdump -i any -c 5 -nn -v \"tcp[13] & 2 != 0\"");
    }

    #[test]
    fn short_options_run_together_are_all_kept() {
        // The LLDP/CDP command as it is usually typed: -n joined to -i, -s and
        // -c with attached values, and a single-quoted filter.
        let p = parse(&cfg(), "tcpdump -ni ix1 -v -s0 -c1 '(ether[12:2]=0x88cc or ether[20:2]=0x2000)'", "powerscale").unwrap();
        assert!(p.notes.is_empty(), "{:?}", p.notes);
        assert_eq!((get(&p, "if"), get(&p, "f.n"), get(&p, "f.v")), (Some("ix1"), Some("1"), Some("1")));
        assert_eq!((get(&p, "f.s"), get(&p, "f.c"), get(&p, "w")), (Some("0"), Some("1"), Some("0")));
        assert_eq!(get(&p, "q"), Some("(ether[12:2]=0x88cc or ether[20:2]=0x2000)"));
        assert_eq!(build(&cfg(), &p.pairs).command, "tcpdump -i ix1 -s 0 -c 1 -n -v \"(ether[12:2]=0x88cc or ether[20:2]=0x2000)\"");

        // With a placeholder, which is how the template is shared.
        let p = parse(&cfg(), "tcpdump -ni <interface> -c 5 port 53", "powerscale").unwrap();
        assert!(p.notes.is_empty(), "{:?}", p.notes);
        assert_eq!((get(&p, "if"), get(&p, "f.n"), get(&p, "f.c"), get(&p, "p")), (Some("<interface>"), Some("1"), Some("5"), Some("53")));
        assert!(build(&cfg(), &p.pairs).command.starts_with("tcpdump -i <interface> -c 5 -n "));

        // Other ways of running them together.
        for (text, expect) in [
            ("tcpdump -nei eth0", vec![("f.n", "1"), ("f.e", "1"), ("if", "eth0")]),
            ("tcpdump -nnvi eth0", vec![("f.nn", "1"), ("f.v", "1"), ("if", "eth0")]),
            ("tcpdump -ieth0 -nvs0", vec![("if", "eth0"), ("f.n", "1"), ("f.v", "1"), ("f.s", "0")]),
            ("tcpdump -nc5 -i eth0", vec![("f.n", "1"), ("f.c", "5"), ("if", "eth0")]),
            ("tcpdump -pni eth0 -w~/tcpdump_capture/x.pcap", vec![("f.p", "1"), ("f.n", "1"), ("if", "eth0"), ("w", "1")]),
        ] {
            let p = parse(&cfg(), text, "linux").unwrap();
            assert!(p.notes.is_empty(), "{text}: {:?}", p.notes);
            for (k, v) in expect {
                assert_eq!(get(&p, k), Some(v), "{text}: {k}");
            }
        }
        // -n and -nn are different options and both survive.
        let p = parse(&cfg(), "tcpdump -n -i eth0", "linux").unwrap();
        assert_eq!((get(&p, "f.n"), get(&p, "f.nn")), (Some("1"), None));
        // A letter the builder does not have is reported; the rest of the run is still read.
        let p = parse(&cfg(), "tcpdump -nXi eth0 port 53", "linux").unwrap();
        assert_eq!(p.notes, ["-X (in -nXi) is not an option the builder knows and was left out."]);
        assert_eq!((get(&p, "f.n"), get(&p, "if"), get(&p, "p")), (Some("1"), Some("eth0"), Some("53")));
        // An unknown option that takes a value has its value skipped, attached or not.
        for text in ["tcpdump -n -y EN10MB -i eth0 port 53", "tcpdump -nyEN10MB -i eth0 port 53"] {
            let p = parse(&cfg(), text, "linux").unwrap();
            assert_eq!(p.notes.len(), 1, "{text}: {:?}", p.notes);
            assert_eq!((get(&p, "f.n"), get(&p, "if"), get(&p, "p"), get(&p, "q")), (Some("1"), Some("eth0"), Some("53"), None), "{text}");
        }
        // A flag that exists but not for this system says so.
        let p = parse(&cfg(), "tcpdump -n -Z root -i lagg0", "powerscale").unwrap();
        assert_eq!(p.notes, ["-Z is not available for PowerScale (OneFS) and was left out."]);
        assert_eq!(get(&p, "if"), Some("lagg0"));
        // Inside an isi_for_array wrapper the closing quote still ends the command.
        let p = parse(&cfg(), "isi_for_array -X 'tcpdump -ni lagg0 -c1 port 53' &", "").unwrap();
        assert_eq!((get(&p, "sc"), get(&p, "if"), get(&p, "f.n"), get(&p, "p")), (Some("all"), Some("lagg0"), Some("1"), Some("53")));
    }

    #[test]
    fn grouped_filter_goes_back_into_the_guided_fields() {
        let p = parse(&cfg(), "tcpdump -i eth0 \\( host 10.0.0.1 or net 10.1.0.0/16 \\) and portrange 8000-8100 and tcp", "linux").unwrap();
        assert_eq!((get(&p, "h"), get(&p, "net")), (Some("10.0.0.1"), Some("10.1.0.0/16")));
        assert_eq!((get(&p, "p"), get(&p, "pr"), get(&p, "q")), (Some("8000-8100"), Some("tcp"), None));
        let p = parse(&cfg(), "tcpdump -i eth0 host 10.0.0.1 and not port 22", "linux").unwrap();
        assert_eq!((get(&p, "h"), get(&p, "q")), (None, Some("host 10.0.0.1 and not port 22")));
    }

    #[test]
    fn placeholders_are_kept_as_placeholders() {
        let text = "mkdir -p /ifs/data/Isilon_Support/<case number>/$(date +%F); isi_for_array -n<nodes> 'tcpdump -i <interface> -s 600 -C 500 -W 5 \
                    -w /ifs/data/Isilon_Support/<case number>/$(date +%F)/x.pcap host <client IP address> or host 10.0.0.1 &'";
        let p = parse(&cfg(), text, "powerscale").unwrap();
        assert!(p.notes.is_empty(), "{:?}", p.notes);
        assert_eq!((get(&p, "if"), get(&p, "n"), get(&p, "sd")), (Some("<interface>"), Some("<nodes>"), Some("<case number>")));
        assert_eq!(get(&p, "h"), Some("<client IP address>,10.0.0.1"));
        let out = build(&cfg(), &p.pairs);
        assert!(out.command.contains("-i <interface>") && out.command.contains("\"host <client IP address> or host 10.0.0.1\""));
        let names: Vec<&str> = out.placeholders.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["<interface>", "<client IP address>", "<nodes>", "<case number>"]);
    }

    #[test]
    fn recurring_captures_survive_a_round_trip() {
        let cfg = cfg();
        for q in [
            "v=1&if=lagg0&w=1&dd=1&lg=1&bg=1&f.s=600&f.G=300&f.W=2&as=cron&ck=daily&ct=01:30",
            "v=1&sc=all&im=all&w=1&dd=1&lg=1&bg=1&f.s=600&f.C=500&f.W=5&as=cron&ck=weekdays&ct=22:05&cd=1,3,5&cn=nightly&ae=delay&aev=2&aeu=hours",
            "v=1&t=linux&if=eth0&w=1&dd=1&lg=1&bg=0&f.s=600&f.G=60&f.W=5&as=cron&ck=hours&ch=6&p=53",
            "v=1&t=linux&if=eth0&w=1&dd=1&lg=0&bg=0&f.c=1000&as=cron&ck=custom&cx=*/15 9-17 * * 1-5&ae=delay&aev=90&aeu=minutes",
        ] {
            let original = build(&cfg, &parse_query(q));
            assert!(original.command.contains("crontab -"), "{q}: {:?}", original.issues);
            let parsed = parse(&cfg, &original.command, "").expect("command found");
            assert!(parsed.notes.is_empty(), "{q}: {:?}", parsed.notes);
            assert_eq!(build(&cfg, &parsed.pairs).command, original.command, "{q}");
        }
    }

    #[test]
    fn text_without_a_capture_command_is_rejected() {
        assert!(parse(&cfg(), "ls -la /ifs", "").is_none());
        assert!(parse(&cfg(), "isi_for_array -X 'killall -SIGINT tcpdump'", "").is_none());
    }

    #[test]
    fn injected_shell_is_never_carried_through() {
        for text in ["tcpdump -i eth0; reboot", "tcpdump -i $(reboot) port 53", "tcpdump -i eth0 -s 600 port 53 | sh", "tcpdump -i eth0 -Z root;id"] {
            let p = parse(&cfg(), text, "linux").unwrap();
            let out = build(&cfg(), &p.pairs);
            assert!(!out.command.contains("reboot") && !out.command.contains("| sh") && !out.command.contains(";id"), "{text}");
        }
    }
}
