# Cursor Jiggler

A tiny, pretty, cross‑platform mouse jiggler. It keeps your computer awake and your status active by nudging the cursor — but only when you're **idle**, so it never gets in the way while you're actually working.

<p align="center">
  <img src="reference/app-icon-white.svg" width="96" alt="Cursor Jiggler icon" />
</p>

## Features

- **Idle‑aware** — only moves the cursor after N seconds of no activity; any input resets the timer.
- **Zen mode** — a 1‑pixel nudge‑and‑return that's practically invisible.
- **Keeps the PC awake** while active (no sleep / screen‑off).
- **System tray** — status in the menu, a green dot on the icon when active, single/double‑click to open.
- **Autostart** at login, optionally **start minimized** to the tray.
- **Auto‑updates** — checks GitHub Releases and updates itself.
- **12 languages**, automatic from your system locale, with light/dark theme.

## Tech stack

- [Tauri 2](https://v2.tauri.app/) (Rust) + [Svelte 5](https://svelte.dev/) + Vite
- Cursor control via [`enigo`](https://crates.io/crates/enigo)

## Development

Prerequisites: [Node.js](https://nodejs.org), [Rust](https://rustup.rs), and the Tauri
system dependencies for your OS (on Windows: the **Desktop development with C++**
workload — MSVC + Windows SDK).

```bash
npm install
npm run tauri dev      # run the app
npm run tauri build    # build installers → src-tauri/target/release/bundle/
```

## Releases & auto‑update

Releases are built by GitHub Actions (`.github/workflows/release.yml`) for Windows,
macOS and Linux whenever a `v*` tag is pushed:

```bash
# bump the version in src-tauri/tauri.conf.json, src-tauri/Cargo.toml and package.json
git tag v1.1.0
git push origin v1.1.0
```

The workflow signs the update artifacts and publishes a draft release containing the
installers and `latest.json`. The running app reads `latest.json` from the latest
release and updates itself.

> **Required repository secret:** `TAURI_SIGNING_PRIVATE_KEY` — the contents of the
> updater private key (`src-tauri/jig.key`, generated with `tauri signer generate`).
> Keep this key safe; losing it means you can no longer ship updates.

## License

MIT © [drxid](https://drxid.ru/)
