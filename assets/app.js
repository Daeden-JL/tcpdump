"use strict";

// The page is a thin form over the Rust server: the server owns defaults,
// validation and command generation (api/build); this file renders the form
// from api/config and keeps the address bar in sync.

const $ = (sel) => document.querySelector(sel);

let config = null;
let state = null; // text fields are kept as the strings the user typed
let flagValues = {}; // key -> value, remembered while a flag is switched off
let buildSeq = 0;
let timer = null;

function el(tag, props, ...kids) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (v === false || v == null) continue;
    if (k === "class") node.className = v;
    else if (k === "text") node.textContent = v;
    else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else if (k in node && k !== "list") node[k] = v;
    else node.setAttribute(k, v);
  }
  for (const kid of kids.flat()) if (kid) node.append(kid);
  return node;
}

async function getJSON(url) {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: ${res.status}`);
  return res.json();
}

const target = () => config.targets.find((t) => t.id === state.target) || config.targets[0];
const flagKey = (f) => f.name.replace(/^-+/, "");
const visibleFlags = () => config.flags.filter((f) => !f.targets.length || f.targets.includes(state.target));
const list = (text) => text.split(/[\s,]+/).filter(Boolean).join(",");
const enc = (v) => encodeURIComponent(v).replace(/%2C/g, ",").replace(/%2F/g, "/").replace(/%3A/g, ":");

function serialise(skip = []) {
  const s = state;
  const p = [["v", "1"], ["t", s.target]];
  if (target().cluster) {
    p.push(["sc", s.scope]);
    if (s.scope === "nodes") p.push(["n", list(s.nodes)]);
  }
  p.push(["im", s.iface_mode]);
  if (s.iface_mode === "named") p.push(["if", list(s.ifaces)]);
  p.push(["w", s.write ? "1" : "0"]);
  if (s.subdir.trim()) p.push(["sd", s.subdir.trim()]);
  p.push(["dd", s.dated ? "1" : "0"], ["lg", s.log ? "1" : "0"], ["bg", s.background ? "1" : "0"]);
  for (const f of visibleFlags()) {
    const k = flagKey(f);
    if (k in s.flags) p.push([`f.${k}`, f.value_type === "Bool" ? "1" : flagValues[k] ?? ""]);
  }
  if (s.at_start !== "now") {
    p.push(["as", s.at_start], ["asv", s.at_start_value.trim()]);
    if (s.at_start === "delay") p.push(["asu", s.at_start_unit]);
  }
  if (s.at_stop !== "none") {
    p.push(["ae", s.at_stop], ["aev", s.at_stop_value.trim()]);
    if (s.at_stop === "delay") p.push(["aeu", s.at_stop_unit]);
  }
  p.push(...estimatePairs());
  if (list(s.hosts)) p.push(["h", list(s.hosts)]);
  if (list(s.ports)) p.push(["p", list(s.ports)]);
  if (list(s.nets)) p.push(["net", list(s.nets)]);
  if (s.proto) p.push(["pr", s.proto]);
  if (s.raw.trim()) p.push(["q", s.raw.trim()]);
  return p.filter(([k]) => !skip.includes(k)).map(([k, v]) => `${k}=${enc(v)}`).join("&");
}

// Estimate inputs: only the ones that are filled in or differ from the default.
function estimatePairs() {
  const s = state;
  const p = [];
  if (s.est_nodes.trim()) p.push(["en", s.est_nodes.trim()]);
  if (s.est_ifaces.trim()) p.push(["ei", s.est_ifaces.trim()]);
  if (s.link_speed !== String(config.estimate.default_link_speed_gbit)) p.push(["ls", s.link_speed]);
  if (s.free_space.trim()) p.push(["fs", s.free_space.trim()]);
  return p;
}

const ESTIMATE_INPUTS = ["est_nodes", "est_ifaces", "link_speed", "free_space"];

// Put the state into the estimate inputs. They live outside the form so they
// are never rebuilt while someone is typing in them.
function syncEstimateInputs() {
  $("#link_speed").replaceChildren(...config.estimate.link_speeds_gbit.map((g) =>
    el("option", { value: String(g), text: `${g} Gbit/s` })));
  for (const id of ESTIMATE_INPUTS) {
    const input = document.getElementById(id);
    if (input.value !== state[id]) input.value = state[id];
  }
}

function showEstimate(est) {
  $("#estimate").hidden = !est;
  if (!est) return;
  $("#est-headline").textContent = est.headline;
  $("#est-headline").className = est.kind;
  $("#est-lines").replaceChildren(...est.lines.map((l) => el("li", { text: l })));
  $("#est-inputs").hidden = est.kind === "none";
  $("#est_nodes-field").hidden = !est.need_nodes;
  $("#est_ifaces-field").hidden = !est.need_ifaces;
  $("#link_speed-field").hidden = !est.need_link;
}

function adopt(serverState) {
  state = {
    ...serverState,
    ifaces: serverState.ifaces.join(", "),
    hosts: serverState.hosts.join(", "),
    ports: serverState.ports.join(", "),
    nets: serverState.nets.join(", "),
  };
  for (const f of config.flags) {
    const k = flagKey(f);
    if (k in state.flags) flagValues[k] = state.flags[k];
    else if (!(k in flagValues)) flagValues[k] = f.default_value;
  }
}

// ---- form ----------------------------------------------------------------

function changed(rerender) {
  if (rerender) renderForm();
  clearTimeout(timer);
  timer = setTimeout(refresh, rerender ? 0 : 150);
}

function textField(id, label, key, hint) {
  return el("label", { class: "field", for: id },
    el("span", { text: label }),
    el("input", {
      id, type: "text", value: state[key], placeholder: hint || "", spellcheck: false,
      autocapitalize: "off",
      oninput: (e) => { state[key] = e.target.value; changed(false); },
    }));
}

function check(id, label, key) {
  return el("label", { class: "check", for: id },
    el("input", { id, type: "checkbox", checked: state[key], onchange: (e) => { state[key] = e.target.checked; changed(true); } }),
    el("span", { text: label }));
}

function radios(name, key, options) {
  return el("div", { class: "radios", role: "radiogroup" }, options.map(([value, label]) =>
    el("label", { class: "check", for: `${name}-${value}` },
      el("input", {
        id: `${name}-${value}`, type: "radio", name, checked: state[key] === value,
        onchange: () => { state[key] = value; changed(true); },
      }),
      el("span", { text: label }))));
}

function section(id, title, ...kids) {
  return el("fieldset", { id: `sec-${id}` }, el("legend", { text: title }), ...kids);
}

// A sensible starting value when a schedule mode is picked.
function scheduleDefault(mode) {
  if (mode === "time") return "01:00";
  if (mode === "delay") return "1";
  if (mode !== "date") return "";
  const d = new Date(Date.now() + 24 * 3600 * 1000);
  const pad = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T01:00`;
}

// One half of the schedule: mode radios plus the input that mode needs.
// `which` is "at_start" or "at_stop"; `off` is the mode meaning "not scheduled".
function scheduleControls(which, off, offLabel, legend) {
  const mode = state[which];
  const valueKey = `${which}_value`;
  const unitKey = `${which}_unit`;
  const modes = [[off, offLabel], ["time", "At a clock time"], ["date", "On a date and time"], ["delay", "After a delay"]];
  const pick = (value) => {
    state[which] = value;
    state[valueKey] = scheduleDefault(value);
    changed(true);
  };
  const onValue = (e) => { state[valueKey] = e.target.value; changed(false); };
  let input = null;
  if (mode === "time") {
    input = el("label", { class: "field", for: valueKey },
      el("span", { text: "Time (24-hour). Runs the next time the clock reaches it." }),
      el("input", { id: valueKey, type: "time", value: state[valueKey], oninput: onValue }));
  } else if (mode === "date") {
    input = el("label", { class: "field", for: valueKey },
      el("span", { text: "Date and time, in the time zone of the system running the capture" }),
      el("input", { id: valueKey, type: "datetime-local", value: state[valueKey], oninput: onValue }));
  } else if (mode === "delay") {
    input = el("div", { class: "field delay" },
      el("label", { for: valueKey }, el("span", { text: "Delay from when the command is pasted" })),
      el("div", { class: "pair" },
        el("input", { id: valueKey, type: "number", min: 1, value: state[valueKey], "aria-label": "Delay amount", oninput: onValue }),
        el("select", { id: unitKey, "aria-label": "Delay unit", onchange: (e) => { state[unitKey] = e.target.value; changed(false); } },
          ["minutes", "hours", "days"].map((u) => el("option", { value: u, text: u, selected: u === state[unitKey] })))));
  }
  return [
    el("p", { class: "sub", text: legend }),
    el("div", { class: "radios", role: "radiogroup", "aria-label": legend }, modes.map(([value, label]) =>
      el("label", { class: "check", for: `${which}-${value}` },
        el("input", { id: `${which}-${value}`, type: "radio", name: which, checked: mode === value, onchange: () => pick(value) }),
        el("span", { text: label })))),
    input,
  ];
}

function flagRow(f) {
  const k = flagKey(f);
  const on = k in state.flags;
  const id = `flag-${k}`;
  const setValue = (v) => { flagValues[k] = v; if (on) state.flags[k] = v; };
  const toggle = (enable) => { if (enable) state.flags[k] = flagValues[k]; else delete state.flags[k]; };

  let input = null;
  if (f.value_type === "Choice") {
    input = el("select", { id: `${id}-value`, "aria-label": `${f.name} value`, onchange: (e) => { setValue(e.target.value); changed(false); } },
      f.choices.map((c) => el("option", { value: c, text: c, selected: c === flagValues[k] })));
  } else if (f.value_type !== "Bool") {
    const v = f.validation || {};
    input = el("input", {
      id: `${id}-value`, "aria-label": `${f.name} value`, value: flagValues[k], spellcheck: false,
      type: f.value_type === "Integer" ? "number" : "text", min: v.min, max: v.max,
      oninput: (e) => { setValue(e.target.value); if (!on) { toggle(true); changed(true); } else { markRecs(); changed(false); } },
    });
  }

  const recs = f.recommendations || [];
  const chips = recs.map((r) => el("button", {
    type: "button", class: "chip", title: r.reason, "data-value": r.value,
    text: r.label ? `${r.value} · ${r.label}` : r.value,
    onclick: () => { setValue(r.value); toggle(true); changed(true); },
  }));
  const reason = el("p", { class: "reason" });
  function markRecs() {
    const hit = on && recs.find((r) => r.value === String(flagValues[k]));
    for (const c of chips) c.classList.toggle("active", !!hit && c.dataset.value === hit.value);
    reason.textContent = hit ? hit.reason : "";
    reason.hidden = !hit;
  }
  markRecs();

  const more = (f.explanation || recs.length) && el("details", {},
    el("summary", { text: recs.length ? `About ${f.name} and which value to pick` : `About ${f.name}` }),
    f.explanation && el("p", { text: f.explanation }),
    recs.length && el("dl", {}, recs.map((r) => [
      el("dt", { text: r.label ? `${r.value} — ${r.label}` : r.value }),
      el("dd", { text: r.reason }),
    ])));

  return el("div", { class: on ? "flag on" : "flag" },
    el("div", { class: "flag-head" },
      el("label", { class: "check", for: id },
        el("input", { id, type: "checkbox", checked: on, disabled: f.required, onchange: (e) => { toggle(e.target.checked); changed(true); } }),
        el("code", { text: f.name }),
        el("span", { text: f.label || "" })),
      input),
    f.description && el("p", { class: "desc", text: f.description }),
    chips.length && el("div", { class: "chips" }, chips),
    reason,
    more);
}

function renderForm() {
  const focused = document.activeElement && document.activeElement.id;
  const t = target();
  const s = state;
  const canAll = t.all_interfaces || t.any_interface;

  const system = section("system", "System",
    el("div", { class: "radios", role: "radiogroup" }, config.targets.map((x) =>
      el("label", { class: "check", for: `target-${x.id}` },
        el("input", { id: `target-${x.id}`, type: "radio", name: "target", checked: x.id === s.target, onchange: () => switchTarget(x.id) }),
        el("span", { text: x.name })))),
    t.description && el("p", { class: "desc", text: t.description }));

  const where = section("where", "Where to capture",
    t.cluster && radios("scope", "scope", [["node", "This node"], ["all", "All nodes"], ["nodes", "Selected nodes"]]),
    t.cluster && s.scope === "nodes" && textField("nodes", "Node numbers", "nodes", t.cluster.nodes_hint),
    radios("iface_mode", "iface_mode", [["named", "Named interface(s)"], ...(canAll ? [["all", "All interfaces"]] : [])]),
    s.iface_mode === "named" && textField("ifaces", "Interface(s), comma separated", "ifaces", t.interface_hint));

  const dirPreview = [t.output_dir, s.subdir.trim().replace(/^\/+|\/+$/g, ""), s.dated ? t.date_subdir : ""].filter(Boolean).join("/");
  const output = section("output", "Output",
    check("write", "Write packets to a file (off prints them to the screen)", "write"),
    s.write && textField("subdir", `Subfolder under ${t.output_dir} (optional)`, "subdir", "e.g. a case number, NDMP, CIFS"),
    s.write && check("dated", "Add a folder for today's date", "dated"),
    s.write && check("log", "Save tcpdump's messages to a .log file next to the capture", "log"),
    s.write && check("background", "Run in the background", "background"),
    s.write && el("p", { class: "desc" }, "Files go to ", el("code", { text: dirPreview })));

  const schedule = section("schedule", "Schedule",
    el("p", { class: "desc", text: "Queue the capture with the at command instead of starting it straight away, and optionally queue the stop as well." }),
    scheduleControls("at_start", "now", "Start now", "Start"),
    scheduleControls("at_stop", "none", "Stop manually", "Stop"),
    (s.at_start !== "now" || s.at_stop !== "none") && t.at_note && el("p", { class: "desc", text: t.at_note }));

  const flags = section("options", "Options", visibleFlags().map(flagRow));

  const filter = section("filter", "Filter",
    el("p", { class: "desc", text: "Leave everything empty to capture all traffic. Hosts and networks are alternatives; the groups are combined with \"and\"." }),
    textField("hosts", "Hosts", "hosts", "192.168.1.1, 192.168.1.2"),
    textField("nets", "Networks", "nets", "172.26.0.0/16"),
    textField("ports", "Ports or ranges", "ports", "445, 2049, 8000-8100"),
    config.filter.protocols.length > 0 && el("label", { class: "field", for: "proto" },
      el("span", { text: "Protocol" }),
      el("select", { id: "proto", onchange: (e) => { s.proto = e.target.value; changed(false); } },
        el("option", { value: "", text: "Any" }),
        config.filter.protocols.map((p) => el("option", { value: p, text: p, selected: p === s.proto })))),
    textField("raw", "Extra filter expression", "raw", "tcp[13] & 2 != 0"),
    config.filter.snippets.length > 0 && el("div", { class: "chips" }, config.filter.snippets.map((sn) =>
      el("button", {
        type: "button", class: "chip", title: sn.expression, text: sn.label,
        onclick: () => { s.raw = s.raw.trim() ? `${s.raw.trim()} or ${sn.expression}` : sn.expression; changed(true); },
      }))));

  $("#form").replaceChildren(system, where, output, schedule, flags, filter);
  syncEstimateInputs();
  markFields(lastIssues);
  if (focused) document.getElementById(focused)?.focus();
}

// ---- output ---------------------------------------------------------------

// ---- problems: flag the fields, offer a fix ---------------------------------

const FIELD_IDS = ["ifaces", "nodes", "subdir", "hosts", "nets", "ports", "proto", "raw", "est_nodes", "est_ifaces", "link_speed", "free_space", "at_start_value", "at_stop_value"];
let lastIssues = [];
let pendingFix = null;

// Outline each setting that has an error and put the message next to it.
function markFields(issues) {
  lastIssues = issues;
  for (const n of document.querySelectorAll(".field-error")) n.remove();
  for (const n of document.querySelectorAll(".invalid")) {
    n.classList.remove("invalid");
    n.removeAttribute("aria-invalid");
  }
  for (const issue of issues) {
    if (issue.level !== "error") continue;
    let input = null;
    let anchor = null;
    if (issue.field.startsWith("f.")) {
      const key = issue.field.slice(2);
      input = document.getElementById(`flag-${key}-value`) || document.getElementById(`flag-${key}`);
      anchor = input && input.closest(".flag-head");
    } else if (FIELD_IDS.includes(issue.field)) {
      input = document.getElementById(issue.field);
      anchor = input && input.closest(".field");
    }
    if (!input || !anchor) continue;
    input.classList.add("invalid");
    input.setAttribute("aria-invalid", "true");
    anchor.after(el("p", { class: "field-error", text: issue.message }));
  }
}

// Ask the server what it would change, then let the user confirm.
async function proposeFix() {
  const result = await getJSON(`api/fix?${serialise()}`);
  const left = result.output.issues.filter((i) => i.level === "error").length;
  pendingFix = result.changes.length ? result : null;
  $("#fix-title").textContent = pendingFix ? "Apply these fixes?" : "Nothing to fix automatically";
  $("#fix-intro").textContent = !pendingFix
    ? "These problems need to be corrected by hand."
    : left
      ? `These changes will be made to your settings. ${left} problem${left === 1 ? "" : "s"} will remain to correct by hand.`
      : "These changes will be made to your settings, and the command will be rewritten to match.";
  $("#fix-changes").replaceChildren(...result.changes.map((c) => el("li", { text: c })));
  $("#fix-apply").hidden = !pendingFix;
  $("#fix-cancel").textContent = pendingFix ? "Cancel" : "Close";
  $("#fix-dialog").showModal();
}

function applyFix() {
  $("#fix-dialog").close();
  if (!pendingFix) return;
  const out = pendingFix.output;
  pendingFix = null;
  buildSeq++;
  flagValues = {};
  adopt(out.state);
  renderForm();
  showOutput(out);
  showNotes([]);
  history.replaceState(null, "", `?${serialise()}`);
}

let generated = ""; // the command as the builder last wrote it

function fitCommand() {
  const box = $("#command");
  box.style.height = "auto";
  box.style.height = `${box.scrollHeight + 2}px`;
}

// Parse and Undo appear only while the box differs from the builder's command.
function markEdited() {
  const box = $("#command");
  const edited = box.value !== generated;
  box.classList.toggle("edited", edited);
  $("#parse").hidden = !edited || !box.value.trim();
  $("#revert").hidden = !edited;
  $("#copy-command").disabled = !box.value.trim();
  fitCommand();
}

function showNotes(notes) {
  $("#parse-notes").hidden = !notes.length;
  $("#parse-notes").replaceChildren(...notes.map((n) => el("li", { text: n })));
}

function showOutput(out, keepText) {
  generated = out.command;
  if (!keepText) $("#command").value = out.command;
  markEdited();
  $("#issues").replaceChildren(...out.issues.map((i) => el("li", { class: i.level, text: i.message })));
  $("#fix").hidden = !out.issues.some((i) => i.level === "error");
  showEstimate(out.estimate);
  markFields(out.issues);
  $("#stop").hidden = !out.stop_command;
  $("#stop-command").textContent = out.stop_command || "";
  $("#stop-note").hidden = !out.command || !!out.stop_command;
}

// Send the edited text to the server, which reads it back into settings.
async function parseCommand() {
  const text = $("#command").value;
  const seq = ++buildSeq;
  const keep = [["t", state.target], ...estimatePairs()].map(([k, v]) => `${k}=${enc(v)}`).join("&");
  const res = await fetch(`api/parse?${keep}`, { method: "POST", body: text });
  if (!res.ok) throw new Error(`api/parse: ${res.status}`);
  const result = await res.json();
  if (seq !== buildSeq) return;
  if (!result.found) {
    showNotes(["No tcpdump command was found in that text, so nothing was changed."]);
    return;
  }
  flagValues = {};
  adopt(result.output.state);
  renderForm();
  // If the parsed settings do not build, keep the text so it can be corrected.
  showOutput(result.output, !result.output.command);
  showNotes(result.notes);
  history.replaceState(null, "", `?${serialise()}`);
}

async function refresh() {
  const query = serialise();
  history.replaceState(null, "", `?${query}`);
  const seq = ++buildSeq;
  try {
    const out = await getJSON(`api/build?${query}`);
    if (seq === buildSeq) {
      showOutput(out);
      showNotes([]);
    }
  } catch (e) {
    console.error(e);
  }
}

// Ask the server to resolve a query into full state, then redraw everything.
async function load(query, updateUrl) {
  const seq = ++buildSeq;
  const out = await getJSON(`api/build?${query}`);
  if (seq !== buildSeq) return;
  adopt(out.state);
  renderForm();
  showOutput(out);
  showNotes([]);
  if (updateUrl) history.replaceState(null, "", `?${serialise()}`);
}

function switchTarget(id) {
  // Drop what only made sense for the old system so the new one's defaults apply.
  const rest = serialise(["t", "sc", "n", "if", "dd", "lg", "bg"]);
  load(`${rest}&t=${enc(id)}`, true);
}

function renderExamples() {
  $("#examples-section").hidden = !config.examples.length;
  $("#examples").replaceChildren(...config.examples.map((ex) =>
    el("article", { class: "example" },
      el("div", { class: "result-head" },
        el("h3", { text: ex.title }),
        el("button", { type: "button", text: "Load", "aria-label": `Load example: ${ex.title}`, onclick: () => { load(ex.query, true); window.scrollTo({ top: 0, behavior: "smooth" }); } })),
      ex.description && el("p", { class: "desc", text: ex.description }),
      el("pre", { text: ex.command, tabindex: 0 }))));
}

async function loadConfig() {
  config = await getJSON("api/config");
  const title = config.site.title || "tcpdump command builder";
  document.title = title;
  $("#title").textContent = title;
  $("#tagline").textContent = config.site.tagline;
  const banner = $("#config-error");
  banner.hidden = !config.error;
  banner.textContent = config.error ? `The configuration file has an error, so the last working version is shown. ${config.error}` : "";
  const links = [[config.site.repo, "Source code"], [config.site.license, "License"]]
    .filter(([l]) => l && l.enabled && l.url)
    .map(([l, fallback]) => el("a", { href: l.url, rel: "noopener", text: l.label || fallback }));
  for (const box of [$("#links"), $("#side-links")]) {
    box.hidden = !links.length;
    box.replaceChildren(...links.map((a) => a.cloneNode(true)));
  }
  renderExamples();
}

async function copy(text, button) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = el("textarea", { value: text });
    document.body.append(ta);
    ta.select();
    document.execCommand("copy");
    ta.remove();
  }
  const old = button.textContent;
  button.textContent = "Copied";
  setTimeout(() => { button.textContent = old; }, 1200);
}

function shareLink() {
  const base = (config.site.url || location.origin + location.pathname).replace(/\/+$/, "");
  return `${base}/?${serialise()}`;
}

async function main() {
  $("#copy-command").addEventListener("click", (e) => copy($("#command").value, e.currentTarget));
  $("#command").addEventListener("input", markEdited);
  $("#command").addEventListener("keydown", (e) => {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && !$("#parse").hidden) parseCommand().catch(console.error);
  });
  $("#parse").addEventListener("click", () => parseCommand().catch(console.error));
  $("#revert").addEventListener("click", () => {
    $("#command").value = generated;
    markEdited();
    showNotes([]);
  });
  window.addEventListener("resize", fitCommand);
  for (const id of ESTIMATE_INPUTS) {
    const input = document.getElementById(id);
    input.addEventListener(input.tagName === "SELECT" ? "change" : "input", () => {
      state[id] = input.value;
      changed(false);
    });
  }
  $("#fix").addEventListener("click", () => proposeFix().catch(console.error));
  $("#fix-apply").addEventListener("click", applyFix);
  $("#fix-cancel").addEventListener("click", () => $("#fix-dialog").close());
  $("#copy-stop").addEventListener("click", (e) => copy($("#stop-command").textContent, e.currentTarget));
  $("#copy-link").addEventListener("click", (e) => copy(shareLink(), e.currentTarget));
  $("#reset").addEventListener("click", async () => {
    flagValues = {};
    await load("", false);
    history.replaceState(null, "", location.pathname);
  });
  $("#form").addEventListener("submit", (e) => e.preventDefault());

  await loadConfig();
  const query = location.search.slice(1);
  await load(query, query !== "");

  // Live config edits: the server announces each reload.
  let seen = config.generation;
  const events = new EventSource("api/events");
  events.addEventListener("config", async (e) => {
    if (Number(e.data) === seen) return;
    seen = Number(e.data);
    $("#theme").href = `theme.css?g=${seen}`;
    await loadConfig();
    await load(serialise(), location.search !== "");
  });
}

main().catch((e) => {
  console.error(e);
  $("#command").placeholder = "The builder could not be loaded. Reload the page to try again.";
});
