# Tauri + React + Typescript

This template should help get you started developing with Tauri, React and Typescript in Vite.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

## Microsoft Store (Windows)

This project is prepared for Microsoft Store Win32 packaging (MSI/NSIS submission flow).

1. Confirm publisher in `src-tauri/tauri.microsoftstore.conf.json` matches your Partner Center publisher display name.
2. Build Store package:

```bash
npm run build:msstore
```

3. Optional build including updater artifacts (for GitHub updater flow):

```bash
npm run build:msstore:updater
```

Notes:
- `build:msstore` uses `--bundles msi,nsis` with `offlineInstaller` WebView2 mode for Store-friendly installers.
- For signed updater builds, provide `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

## Windows Architectures

Build 64-bit:

```bash
npm run build:win:x64
```

Build 32-bit:

```bash
npm run build:win:x86
```
