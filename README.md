# KaizeNotch

A pomodoro timer and habit tracker that sits in a small pill at the top of your screen.
Hover the pill to open it, tap an activity to start, tick off habits, and open a report of your week.

*Kaizen* (改善) means small steady improvement: 1% better every day.

KaizeNotch is the Windows version of [StudyNotch](https://github.com/nickaqmal02/ApexAgent) (which lives
in the MacBook notch). Both keep the same files, so a data folder can move between them.

## Install on Windows

**Scoop**

```
scoop bucket add nickaqmal02 https://github.com/nickaqmal02/scoop-bucket
scoop install kaizenotch
```

**Installer:** download `KaizeNotch_<version>_x64-setup.exe` (or the `.msi`) from the
[latest release](https://github.com/nickaqmal02/KaizeNotch/releases/latest) and run it.

The app is not code-signed yet, so Windows SmartScreen may say "Windows protected your PC".
Choose **More info → Run anyway**. Check the file against `SHA256SUMS.txt` in the release if you want to be sure.

It needs Windows 10 or 11. The installer fetches Microsoft's WebView2 runtime if it is missing
(Windows 11 already has it).

## Use it

| Do this | What happens |
|---|---|
| Hover the pill | The panel opens |
| Tap a tile | A pomodoro starts. Activities with choices ask first (Pray → Asar) |
| ✓ on a tile | "I already did it": logs the habit without running the timer |
| Pencil on a tile | Change the name, length, break, tab, choices, or habit / time only |
| **+** after the tabs | Add your own category |
| Tray icon | Open My Report, Share My Report on the Web, Start at Login, Quit |

The four built-in tabs are Academic, Health, Soul and Personal. Academic activities share one
"Study" habit; every other activity is its own habit unless you turn that off.

## Your data

Everything stays on your computer, in plain files:

| | Folder |
|---|---|
| Windows | `%APPDATA%\KaizeNotch\` |
| macOS (for development) | `~/Library/Application Support/KaizeNotch/` |

`subjects.json` (activities), `categories.json` (your tabs), `study_sessions.csv` (one row per
session) and `report.html` (the report, rebuilt after every session).

## Sharing (off until you turn it on)

Tray icon → **Share My Report on the Web…** gives you a private link like
`https://…cloudfront.net/r/AbC123xyz789` that you can open on your phone or send to a friend.

- No account, no password. The app gets a random link and a secret; the secret is kept in
  Windows Credential Manager.
- Only the report summary is uploaded (habits, streaks, totals). The list of sessions with their
  start and stop times never leaves your computer.
- It uploads at most every 15 minutes, and only when something changed.
- **Stop Sharing** deletes the copy from the server and the link stops working.

## Build from source

You need [Rust](https://rustup.rs) and Node.js 20 or newer.

```
npm ci
npx tauri dev        # run it
npx tauri build      # Windows: makes the .msi and setup .exe
```

Tests and checks (run inside `src-tauri/`):

```
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo test -- --ignored live_share_round_trip   # talks to the real share service
```

Environment variables for testing: `KAIZENOTCH_DATA` (another data folder), `KAIZENOTCH_API`
(another share service; https or `http://127.0.0.1`).

## How it is built

| Part | Where | Job |
|---|---|---|
| Page | `ui/` (plain HTML, CSS, JS; no bundler) | The pill, the panel and the running timer |
| Store | `src-tauri/src/store.rs` | Reads and writes the data files |
| Report | `src-tauri/src/report.rs` | Builds the report data (same output as StudyNotch) |
| Share | `src-tauri/src/share.rs` | Register, upload and stop, with the secret in the credential store |
| App | `src-tauri/src/lib.rs` | Window placement, hover, tray menu, commands for the page |

Built with [Tauri 2](https://tauri.app): Rust underneath, the system web view for the interface.
The report template (`src-tauri/assets/report_template.html`) is a copy of
`scripts/study_report_template.html` in the StudyNotch repo; copy it again when that one changes.

## Releasing

1. Set the new version in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `package.json`.
2. `git tag v0.2.0 && git push origin v0.2.0`. The Release workflow builds and publishes the files.
3. Update `bucket/kaizenotch.json` in the scoop-bucket repo (version, url, hash from `SHA256SUMS.txt`).

## License

MIT
