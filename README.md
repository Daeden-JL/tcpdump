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

## Configuration

[`tcpdump.toml`](tcpdump.toml) is commented throughout. The main parts:

| Section | Purpose |
| --- | --- |
| `[site]` | `url` is the public address used for share links. Change it to host the app anywhere; leave it empty to use the address the visitor is on. `[site.repo]` and `[site.license]` are footer links; set `enabled = false` to hide one. |
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
| `GET /api/config` | The current config with example commands filled in |
| `GET /api/events` | Server-sent event on each config reload |
| `GET /healthz` | `ok` |

## Development

```bash
cargo test
```

Command generation and validation live in `src/builder.rs`, the config schema
in `src/config.rs`, the server and file watcher in `src/main.rs`, and the page
in `assets/`.
