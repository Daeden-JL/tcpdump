# tcpdump.dev

A small Rust web app that helps people build, adjust and share `tcpdump`
commands, mainly for Dell PowerScale (OneFS) and Linux.

- **One TOML file drives everything**: systems, flags, explanations,
  recommended values, filter shortcuts and examples.
- **Live edits**: the server watches the file. Save it and every open page
  updates; no restart.
- **Shareable**: the whole setup is stored in the page address, so a copied
  link shows the receiver exactly what the sender configured.
- **PowerScale wrappers**: node selection with `isi_for_array`, interface
  loops, dated folders, log files, backgrounding and the matching stop command.

## Run

```bash
cargo run --release
```

Then open <http://127.0.0.1:8080>. The config path is the first argument or
`TCPDUMP_DEV_CONFIG` (default `./tcpdump.toml`). The listen address comes from
`LISTEN`, then `site.listen`, then `127.0.0.1:8080`.

With Docker:

```bash
docker build -t tcpdump-dev .
```

```bash
docker run -p 8080:8080 -v "$PWD:/config" tcpdump-dev
```

Mount the directory, not the single file, so editors that replace the file on
save are still picked up. Put a TLS-terminating reverse proxy (Caddy, nginx)
in front for a public site. The page uses relative addresses, so it also works
under a path prefix as long as the proxy strips the prefix.

The page loads its script, stylesheet and icon from addresses that contain a
fingerprint of their content (`/static/<fingerprint>/…`), so browsers and CDNs
can cache them indefinitely and a new build is never shown with old styling.
The page itself is served with `Cache-Control: no-store`; if you put a CDN in
front, do not configure it to cache HTML.

The page shows the app version above the repository link: the version from
`Cargo.toml`, plus the short git commit the binary was built from when git
was available at build time (for example `v1.0.8 (fecb28f)`).

## Configuration

[`tcpdump.toml`](tcpdump.toml) is commented throughout. The main parts:

| Section | Purpose |
| --- | --- |
| `[site]` | `url` is the public address used for share links. Change it to host the app anywhere; leave it empty to use the address the visitor is on. `[site.repo]` and `[site.license]` are footer links; set `enabled = false` to hide one. |
| `[theme]` | `mode` (`auto`, `light` or `dark`) and the hex colours for `[theme.light]` and `[theme.dark]`. Any colour left out keeps its default. |
| `[estimate]` | Link speeds offered for the worst-case estimate of timed captures, and the default. |
| `[[targets]]` | A system: tcpdump command, output folder, file and log name templates, interface defaults, and the optional `[targets.cluster]` wrappers. |
| `[[flags]]` | A tcpdump option: type, default, limits, `dependencies`, `exclusions`, a one-line `description`, a longer `explanation`, and `[[flags.recommendations]]`. |
| `[filter]` | Protocol list and one-click filter snippets. |
| `[[examples]]` | Saved setups, stored as the query string of a share link. |

A flag with recommended values:

```toml
[[flags]]
name = "-s"
label = "Snap length"
description = "Bytes captured from each packet. 0 means the whole packet."
explanation = "Longer background, shown on demand."
value_type = "Integer"
default_value = 900
enabled = true
validation = { min = 0, max = 262144 }

[[flags.recommendations]]
value = 600
label = "SMB / NFS operations"
reason = "Holds the protocol headers of most SMB and NFS calls without the file data."
```

`-i` and `-w` are generated from the target settings, so they are not listed
as flags. `dependencies` is satisfied when at least one listed flag is on
(`-W` needs `-C` or `-G`).

If an edit does not parse or validate (unknown keys, a flag pointing at a
missing flag, an example that no longer builds), the last working version
stays live and the page shows the error.

To add an example, set the builder up the way you want and copy everything
after the `?` in the address bar into `query`.

## Summary under the command

Below the command is a short plain-language summary of what it does as
configured: where it captures, which traffic, what is saved and how the files
are limited, how it runs, and when it starts and stops if scheduled. It is
written by the server from the same settings as the command, so the two
always agree.

## Editing the command

The command box is editable. Change it, or paste a command from elsewhere,
and a **Parse** button appears; it reads the text back into the settings.
Anything the builder has no setting for (an unknown option, a folder outside
the configured one, a `<placeholder>`) is listed under the box and left out,
and the box then shows the builder's own version of the command.

After a Parse, a **What Parse changed** panel shows the text you entered and
the builder's command one above the other, with the differences marked:
struck through where something you wrote is not in the result as written,
underlined where something is new, and dotted where a word only moved. The
notes underneath say why anything was left out or replaced. Options written
together (`-ni eth0`, `-nvs0`) are understood and kept.

### Reporting a parsing problem

After a Parse, a **Report a parsing problem** button opens a form with the
text that was entered, what Parse produced and the notes it showed, plus a
box for what was expected. Everything can be edited before it is sent, so
private details can be removed. `[feedback]` in the TOML chooses where it
goes:

| `mode` | What happens |
| --- | --- |
| `file` (default) | Saved privately on the server, one JSON object per line, in `feedback.file` next to the config. |
| `github` | Opens a pre-filled issue on `feedback.github_repo` (default: the repo link). Public; needs a GitHub account. |
| `email` | Opens the visitor's mail program, addressed to `feedback.email`. |
| `off` | No button. |

In `file` mode the server accepts at most 20 reports a minute, 16 KB per
report, and stops when the file reaches 5 MB. Read the file with any text
tool, for example `jq . parse-feedback.jsonl`.

## Invalid values

A setting with a problem is outlined in the form with the message beside it,
and the same problems are listed under the command. This applies whether the
value was typed, parsed from a command or came from a shared link.

**Fix problems…** shows what it would change and applies it only after you
confirm:

- a number out of range moves to the nearest limit, unless the flag sets
  `validation.over_max` (snap length does: a value above the maximum becomes `0`,
  whole packet);
- a value that is not a number, or not an allowed choice, goes to the flag's default;
- a flag that clashes with another, or lacks the flag it depends on, is switched off;
- invalid interfaces, hosts, networks, ports, subfolders and filter text are removed.

## Saving printed output, and a time limit

With "Write packets to a capture file" off, tcpdump prints packets as text.
**Save the printed packets to a text file** redirects that text to a file per
interface in the capture folder (`text_name` and `text_redirect` in the TOML),
creating the folder first.

**Give up after this many seconds** wraps the capture in `timeout`
(`timeout_wrap` in the TOML). It pairs well with a packet count: `-c 1` waits
for as long as it takes, which is forever on an interface where the packet
never arrives. A time limit also gives a capture file a worst-case size in
the estimate.

Two more optional fields appear whenever something is saved:

- **File name** overrides the standard name for that command. It takes plain
  characters plus `{hostname}`, `{iface}` and `{timestamp}`, for example
  `{hostname}_{iface}.lldp.txt`. A name without `{iface}` or `{hostname}` is
  flagged when several interfaces or nodes would write to the same file.
- **Pause after creating the folder** adds `sleep N` between `mkdir` and the
  capture, to give a new folder time to appear (`pause` in the TOML).

Parse reads all of these back, including a `timeout` inside a cluster
wrapper. A pasted file name is kept when it can be written with those three
placeholders; `${HOST}`, `$(hostname)` and the loop variable are recognised.

## Scheduling with `at`

The **Schedule** section queues the capture with `at` instead of starting it
straight away, and can queue the stop command as a second job. Each can be a
clock time (`at 01:00`), a date and time (`at -t 202611120100`) or a delay
(`at now + 2 hours`). The capture is wrapped in a here-document so it runs
exactly as written:

```bash
at 01:00 <<'EOF'
mkdir -p /ifs/data/Isilon_Support/$(date +%F); tcpdump -i lagg0 ...
EOF
```

`at` jobs run under `/bin/sh`, so inside a job the log redirect comes from
`at_log_redirect` (`>> file 2>&1`) instead of `log_redirect`. Per system,
`at_command` sets how the job is queued (`sudo at` on Linux) and `at_note` is
the text shown beside the settings. Parse reads these jobs back.

### Queuing an `at` job from a script

With a scheduled start, **Put the capture in a script file** writes the
capture to `tcpdump_<job name>.sh` and queues that file with `at -f`, instead
of passing the command inline. The script stays on disk, so it can be read,
edited and queued again. `at_script_install` and `script_remove` in the TOML
control how it is written and deleted.

### Listing and removing scheduled jobs

Whenever a capture is scheduled, the result panel shows the commands for
looking after it: `atq`, `at -c <job number>` and `atrm <job number>` for `at`
jobs, or `crontab -l` and a ready-made removal command for a recurring
capture. They come from `at_list`, `at_show`, `at_remove`, `cron_list` and
`cron_remove` in the TOML.

### Recurring captures with cron

Ticking **Recurring capture (repeat with cron)** shows the cron settings and
installs a recurring capture: every
day at a time, on chosen weekdays, every few hours, or a custom five-field
cron expression. Cron treats `%` specially and tcpdump file names are full of
them, so the capture is written into a small script and the crontab gets one
line that runs it:

```bash
cat > /ifs/data/Isilon_Support/tcpdump_nightly.sh <<'EOF'
#!/bin/sh
mkdir -p /ifs/data/Isilon_Support/$(date +%F); tcpdump -i lagg0 ...
EOF
chmod +x /ifs/data/Isilon_Support/tcpdump_nightly.sh
(crontab -l 2>/dev/null | grep -v '/ifs/data/Isilon_Support/tcpdump_nightly.sh'; echo '30 1 * * 1-5 /ifs/data/Isilon_Support/tcpdump_nightly.sh') | crontab -
```

Running it again replaces the earlier line instead of adding a second one.
A recurring capture can be stopped a set time after each start (a `sleep`
inside the script). The page warns when a recurring capture would never end
by itself, and that files accumulate from run to run. `cron_install`,
`cron_remove` and `cron_dir` in the TOML control how and where it is installed.

## Placeholders

Interfaces, node numbers, the subfolder, hosts, networks and ports can be
left open as a placeholder such as `<interface>` or `<client IP address>`
(type it, or switch on the "Leave open as ..." toggle under the field;
switching it off puts the previous value back). The toggle is on for a
placeholder of any name, so `<mgmt interface>` counts too. The command
then shows the placeholder, and anyone opening the link gets a **Fill in
before running** panel at the top with one box per placeholder. What they type
is checked like any other value and put into the command; the settings keep
the placeholder, so the same link can be handed on again.

## Interfaces left out of "All interfaces"

On PowerScale, "All interfaces (external)" loops over the output of
`all_interfaces` in the TOML, which contains `egrep -v "{exclude}"`. The
names to leave out start as `all_interfaces_exclude` and can be changed on the
page in the **Leave out interfaces** field; the value is kept in the share
link. Parse reads the list back from a pasted command, so a command that
differs only in those names is kept as written.

## Capture size estimate

The result panel shows how much disk the capture can use:

- **Rotation by size** (`-C` with `-W`) or a **packet count** gives an exact
  upper limit.
- **Rotation by time** (`-G` with `-W`) depends on traffic, so the figure is a
  worst case: a link of the chosen speed completely full in both directions.
  A second line shows what the snap length saves on full-size frames.
- For that worst case the link speed is taken from each interface's name
  where it is recognised (`ix0` is 10 Gbit/s, `mlxen0` 40 Gbit/s), using the
  `interface_speeds` table under `[estimate]`. The link-speed dropdown is
  shown only for names that are not in the table, such as `lagg0`.
- When the command runs on all nodes or loops over all interfaces, their
  number is not in the command, so the box asks for a count.
- A warning appears when nothing caps the capture (no `-W`, or no rotation),
  and when the estimate is larger than the free space entered.

Flags are matched to these meanings with `role` in the TOML, and the link
speeds offered come from `[estimate]`. The counts, link speed and free space
are stored in the share link but never change the command.

## Safety

Generated commands are pasted into root shells, and links can come from
anyone. Every value from the address is checked against a strict character
set on the server, filters with shell characters are quoted, and no command
is shown while any error is outstanding. Templates in the TOML file are
trusted as written.

## API

| Path | Returns |
| --- | --- |
| `GET /api/build?<query>` | JSON: `command`, `stop_command`, `issues`, resolved `state` |
| `POST /api/parse?t=<target>` | Body is command text. JSON: `found`, `notes`, and `output` in the same form as `/api/build` |
| `GET /api/fix?<query>` | JSON: `changes` (what would be repaired) and `output` for the repaired settings |
| `POST /api/feedback` | Saves a parse report (JSON `expected`, `report`) when `feedback.mode` is `file` |
| `GET /theme.css` | The colours from `[theme]` as CSS variables |
| `GET /api/config` | The current config with example commands filled in |
| `GET /api/events` | Server-sent event on each config reload |
| `GET /healthz` | `ok` |

## Development

```bash
cargo test
```

Command generation and validation live in `src/builder.rs`, reading a command
back in `src/parse.rs`, the config schema
in `src/config.rs`, the server and file watcher in `src/main.rs`, and the page
in `assets/`.
