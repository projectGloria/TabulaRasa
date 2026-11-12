# Tabula Rasa

> **Tabula Rasa** *(noun)*: A clean slate. An uninstaller and nothing else.

A fast, lightweight, and bloat-free Windows uninstaller built with **Rust** and **egui**.

---

## Why Tabula Rasa?

Many popular Windows uninstallers bundle heavy utilities—startup managers, junk file cleaners, browser extension managers, and registry defragmenters. 

**Tabula Rasa** is a clean reimplementation focused strictly on doing one job with precision: **uninstalling software and removing what it leaves behind**. Its registry discovery and uninstall mechanics are based on the reverse-engineered core routines of HiBit Uninstaller, re-architected from the ground up in modern Rust with safety guardrails.

---

## Features

- **Accurate Registry Discovery**: Scans 64-bit (`HKLM`), 32-bit (`HKLM\WOW6432Node`), and per-user (`HKCU`) uninstall locations using native 64-bit access masks (`KEY_WOW64_64KEY`).
- **MSI Awareness**: Automatically identifies Windows Installer packages and invokes `MsiExec.exe /X{GUID}`.
- **Smart Metadata Parsing**:
  - Normalizes multiple date formats and falls back to registry key `FILETIME` last-write timestamps when install dates are omitted.
  - Automatically calculates install directory disk usage when uninstaller sizes are missing.
  - Dual-blits native PE icon resources over black and white surfaces to reconstruct accurate 32x32 RGBA alpha channels.
- **Active Process Detection & Termination**:
  - Uses Windows ToolHelp process snapshots to discover running tray agents or background updater processes originating from the target install folder.
  - Provides a one-click action to close locking processes before removal so file deletions succeed without "Access Denied" errors.
- **Precision Leftover Scanning**:
  - Exact name-stem matching (strips architecture tags like `(x64)`, `32-bit`, `version`, and version numbers).
  - Strict generic-word blacklist prevents common directory names from ever being flagged.
  - Cross-references directory paths with all other installed programs; shared directories are never deleted.
  - Comprehensive coverage across Program Files, ProgramData, AppData (Local, Roaming, Programs), Start Menu, and Desktop shortcuts.
- **Non-Destructive & Reversible**:
  - Deleted files and shortcuts are moved to the **Windows Recycle Bin** via shell recycling.
  - Registry keys are automatically exported to `.reg` backup files under `%LOCALAPPDATA%\Tabula Rasa\Backups` before deletion.
- **Safety Guardrails**:
  - Enforces absolute path requirements and prohibits drive roots (`C:\`, `D:\`).
  - Prohibits system directories (`Windows`, `System32`, `SysWOW64`, `Users`, `Recovery`, `System Volume Information`, etc.).
  - UAC elevation integration: seamlessly warns when running as a standard user and provides one-click admin restart.

---

## Keyboard Shortcuts

| Shortcut | Action |
|---|---|
| <kbd>F5</kbd> | Refresh program list |
| <kbd>Ctrl</kbd> + <kbd>F</kbd> | Focus search filter |
| <kbd>↑</kbd> / <kbd>↓</kbd> | Navigate program list |
| <kbd>Enter</kbd> / <kbd>Delete</kbd> | Uninstall selected program |
| <kbd>Esc</kbd> | Close active modal dialog |

---

## Building from Source

### Prerequisites
- [Rust toolchain](https://rustup.rs/) (edition 2024 / 1.85+)
- Windows 10 or 11 (x86_64)

### Build & Run
```powershell
# Run the application
cargo run --release

# Run the test suite
cargo test
```

The compiled binary will be placed at `target/release/TabulaRasa.exe`. Application icon and Windows version metadata are embedded directly via `build.rs`.

---

## Project Structure

```
TabulaRasa/
├── src/
│   ├── main.rs       # Application state, egui dark UI, modal flows, event loop
│   ├── programs.rs   # Registry scanning, MSI resolution, date & size calculation
│   ├── leftovers.rs  # Leftover scanner, safety filters, Recycle Bin & reg backup
│   └── system.rs     # Win32 ShellExecute, UAC elevation, icon extraction, process snapshots
├── assets/           # Application icons (64x64, 256x256 PNGs)
├── build.rs          # Embeds Windows PE icon and product metadata
├── Cargo.toml        # Dependencies and build profile configuration
└── MECHANICS.md      # Documentation of reverse-engineered uninstall mechanics
```

---

## License

MIT License. See project metadata for details.
