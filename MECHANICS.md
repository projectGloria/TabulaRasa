# Tabula Rasa — how the uninstaller works

Read from the original program's code (HiBit Uninstaller, a 32-bit Delphi program) and
then written fresh for Tabula Rasa. Only the uninstaller part was kept; the Tools menu
(registry cleaner, junk files, startup manager, etc.) was left out on purpose.

## Finding installed programs — scan routine at 0x850e4c

| Rule | Value | Source |
|---|---|---|
| Places read, in order | `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`, then `HKLM\SOFTWARE\WOW6432Node\...\Uninstall`, then `HKCU\SOFTWARE\...\Uninstall` | 0x851152, 0x851191, 0x851938 (HKEY 0x80000002 / 0x80000001) |
| Registry access | KEY_READ + 64-bit view (0x20119) | 0x85112a |
| An entry is listed only if it has | `UninstallString` | 0x851229 |
| Hidden as a system component when | `SystemComponent` = 1 (DWORD), or no `DisplayName` | 0x851279–0x851298 |
| Name | `DisplayName`, or the key name when missing | 0x8512db–0x851352 |
| Publisher | `Publisher` | 0x8513db |
| Install folder | `InstallLocation`; else folder of the `UninstallString` program; else folder of `DisplayIcon` | 0x851425–0x8514e2 |
| Install date | `InstallDate` (yyyymmdd) | 0x851522 |
| Windows Installer programs | `WindowsInstaller` = 1 and key is a `{GUID}` → uninstall with `MsiExec.exe /X{GUID}` | 0x8516c6–0x85177d |
| Size | `EstimatedSize` × 1024 (value is in KB) | 0x8517c7 (multiply by 0x400) |
| Size when not stated | size of the install folder | 0x85188a / 0x851898 |
| Size when neither | "Unknown" | 0x851906 |
| Status bar | number of items and total size | 0x854703–0x8547b8 |
| Extra details | `Comments`, `HelpLink`, `URLInfoAbout`, `URLUpdateInfo`, registry path shown as `HKEY_LOCAL_MACHINE\...` | same routine 0x850e4c, after the size step |

## Before uninstalling — routine at 0x864758

The original checks whether another installed program uses the same install folder and,
if so, warns ("leftovers will… are you sure you want to continue", Yes = 6 continues).
Tabula Rasa shows the same warning and never offers that shared folder as a leftover.

## Choices made for Tabula Rasa (estimated — not read from the original)

- Install date falls back to the registry key's last-change date when `InstallDate` is missing.
- Duplicate entries with the same name and version are shown once.
- `DisplayVersion` is shown as its own column.
- Leftovers are matched by exact folder / key names only: the install folder, folders with the
  program's name in Program Files, ProgramData, AppData (Roaming, Local, Local\Programs),
  Start menu folders and shortcuts, desktop shortcuts, and `SOFTWARE\<name>` /
  `SOFTWARE\<publisher>\<name>` keys. Protected system folders are never offered.
- Files go to the Recycle Bin; registry keys are exported to a `.reg` backup in
  `%LOCALAPPDATA%\Tabula Rasa\Backups` before deletion.
