# Stempeluhr

> Moin! Wie man sicherlich sieht, ist die App völlig gevibecoded. Es handelt sich hier also nicht um maintainte und zugängliche Open-Source-Software für die allgemeine Nutzung, sondern nur um ein kleines Tool, das ich basteln gelassen habe, um in meinen eigenen Workflow zu passen. Wenn es das auch für dich tut, feel free, die App zu installieren, aber erwarte nicht zu viel :)

A small, quiet GNOME app that starts and stops time tracking in
[Kimai](https://www.kimai.org/). Plus a plain history of the last 30 days where
you can fix or delete an entry. No reports, no statistics.

- Rust, GTK 4, libadwaita, Blueprint, Meson + Cargo, packaged as a Flatpak
- English source strings, German translation through gettext
- GPL-3.0-or-later

> **Placeholders:** the app name _Stempeluhr_ and the app ID
> `de.example.Stempeluhr` are placeholders. To rename, replace the ID in
> `meson.build`, `src/config.rs`, `src/resources.gresource.xml`,
> `data/*` (file names and contents), `src/ui/*.blp` (`de.example.Stempeluhr`
> icon name), the Flatpak manifest and `src/application.rs` (`RESOURCE_PREFIX`).

## Design in one paragraph

**Kimai is the single source of truth.** The window only ever shows what the
server last confirmed. If the server can't be reached, the last known state is
shown dimmed and frozen, marked "Not confirmed" with the time it is from.
The elapsed time is always computed from the server's `begin` (corrected by the
clock offset from the `Date` header), never counted up locally. The app polls
`GET /api/timesheets/active` on window map, on focus (throttled, 5 s), every
45 s while the window is visible, on network changes, after resume from
suspend, and before every start/stop. Writes are idempotent and conflict-safe:
stopping something already stopped elsewhere succeeds quietly; starting while
something runs creates nothing; `POST`/`PATCH` are never retried — after an
uncertain failure the server is asked what happened instead.

There is **no background process**. When the last window closes the app quits.
The only windowless runs are short D-Bus activations (notification button,
`gapplication action`): start, do the thing, quit.

## Layout

| Path           | What                                                                                                                                |
| -------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `core/`        | GTK-free crate: `kimai` (API client), `state` (state machine), `engine` (sync logic). Tested against a mock server with `wiremock`. |
| `src/`         | The GTK app: window, dialogs, notification, portals                                                                                 |
| `src/ui/*.blp` | Blueprint UI (composite templates)                                                                                                  |
| `data/`        | desktop file, metainfo, GSettings schema, D-Bus service, icons                                                                      |
| `po/`          | gettext (`de`)                                                                                                                      |
| `build-aux/`   | `mock-kimai.py` (fake server for trying things), helper scripts                                                                     |

## API token

In Kimai open your **profile → API** and create an **API token**
(Kimai 2.x). Paste it in the connect dialog together with the server address
(`https://kimai.example.com`; a trailing slash or `/api` is fine). The
connection is tested before anything is saved. The token is stored in the
system keyring (Secret portal inside the Flatpak) — never in GSettings, never
in logs. Revoking the token in Kimai moves the app to "Sign-in required".

## Build and install

### Flatpak

Local build and install for the current user (no crate list needed; the build
may use the network):

```sh
flatpak install flathub org.gnome.Platform//51 org.gnome.Sdk//51 \
    org.freedesktop.Sdk.Extension.rust-stable//26.08
build-aux/flatpak-install.sh        # uses flatpak-builder, or the copy inside GNOME Builder
flatpak run de.example.Stempeluhr
```

Reproducible/offline build (what CI does, see `.github/workflows/flatpak.yml`):
`build-aux/update-cargo-sources.sh` (needs `flatpak-cargo-generator.py`) writes
`build-aux/cargo-sources.json`, then
`flatpak-builder --user --install --force-clean _flatpak build-aux/de.example.Stempeluhr.yaml`.

Finish args are minimal: network, IPC, Wayland/X11 fallback, DRI. No file
system access. Optionally add `--system-talk-name=org.freedesktop.login1` to
receive logind's wake-up signal; without it, wake-ups are detected through a
wall-clock jump and the network monitor.

### GNOME Builder

Open the folder, pick `build-aux/de.example.Stempeluhr.local.yaml`, run.

### Plain Meson (development)

Needs GTK 4, libadwaita ≥ 1.8, `blueprint-compiler`, Rust, gettext.

```sh
meson setup build -Dprefix=$PWD/install
meson compile -C build
meson test -C build          # core tests, desktop file, schema, metainfo validation
meson install -C build
build-aux/run-dev.sh         # run from the build directory
RUST_LOG=stempeluhr=debug,stempeluhr_core=debug build-aux/run-dev.sh
```

Try it without a Kimai server: `build-aux/mock-kimai.py` (token `testtoken`,
`http://127.0.0.1:8765`). Its `/__stop`, `/__start`, `/__offline=1`, `/__skew=N`
endpoints simulate things happening elsewhere.

Tests only for the logic: `cargo test -p stempeluhr-core`.
Lints: `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.

## History

The history page (header bar button, `Ctrl+H`) lists finished entries of the
last 30 days, grouped by day with a daily total. Tap an entry to change project,
activity, description, start and end time, or to delete it. Like everything
else it shows only what the server returned: after a change the list is loaded
again, and `PATCH`/`DELETE` are sent once and never retried automatically. Times
are shown in your system time zone and sent in the time zone of your Kimai
user. Entries that span midnight keep their original dates when edited.

## Actions and shortcuts

| Action       | Effect                                                                                 |
| ------------ | -------------------------------------------------------------------------------------- |
| `app.start`  | Start with the window's selection, or the last used project/activity (see preferences) |
| `app.stop`   | Stop what is shown / what the server says runs                                         |
| `app.toggle` | Stop if running, otherwise start                                                       |

All run without a window and check the server first. Inside the window:
`Ctrl+Return` toggles, `Ctrl+R`/`F5` refreshes, `Ctrl+H` opens the history, `Ctrl+,` opens preferences,
`Ctrl+?` the shortcut overview, `Ctrl+W` closes.

### Your own keyboard shortcut

Settings → Keyboard → Custom Shortcuts → add one with the command

```sh
gapplication action de.example.Stempeluhr toggle
```

(for the Flatpak: `flatpak run --command=gapplication de.example.Stempeluhr action de.example.Stempeluhr toggle`,
or simply `flatpak run de.example.Stempeluhr` plus the D-Bus call above, since
the app is D-Bus activatable). Alternatively enable the optional **global
shortcut** in the preferences (GlobalShortcuts portal); that only works while
the app is running.

## Status

Verified here: core tests (47, including the sync/conflict scenarios), clippy
and rustfmt clean, `meson test` (desktop file, schema, metainfo), build and run
against the mock server including headless D-Bus activation.

Not verified in this environment: `flatpak-builder` build, `flatpak-builder-lint`
(neither tool was available), and behaviour against a real Kimai instance —
the OpenAPI document of an instance (`<server>/api/doc`) requires a login, so the
client follows the documented API and parses tolerantly (ids or expanded objects,
PHP-style `+0100` offsets). Please run it once against your instance and check
especially: the `begin` format accepted by `POST /api/timesheets` (sent in the
user's time zone from `/api/users/me`), and the shape of `/api/timesheets/recent`.

Phase 2 (GNOME Shell search provider) is not implemented.
