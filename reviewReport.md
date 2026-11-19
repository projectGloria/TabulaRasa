# Tabula Rasa — Complete Codebase Review Report

## Architectural & Data-Flow Overview

Before detailing the findings by severity, below is the end-to-end trace of how data, UI state, and image processing flow across every file in the repository:

* **Build & Packaging (`Cargo.toml:1-22`, `build.rs:1-12`, `.gitignore:1-11`)**:
  * `build.rs:1-12` embeds `Tabula Rasa.ico` and PE version metadata into the `TabulaRasa.exe` binary via `winresource`.
  * `main()` (`src/main.rs:973-1004`) parses `--screenshot <path>`, decodes `assets/icon-256.png` (`src/main.rs:981-986`) for the window icon, configures `eframe::NativeOptions`, and initializes `App::new` (`src/main.rs:89-125`), which decodes `assets/icon-64.png` (`src/main.rs:92-100`) as the toolbar logo texture and triggers the initial background scan via `App::refresh` (`src/main.rs:123`).
* **Registry Discovery & Metadata Pipeline (`src/programs.rs:1-439`, `src/main.rs:127-194`)**:
  * `App::refresh` (`src/main.rs:127-166`) spawns a worker thread that calls `programs::scan()` (`src/programs.rs:321-347`).
  * `programs::scan()` iterates over `HKLM\...\Uninstall` (64-bit), `HKLM\SOFTWARE\WOW6432Node\...\Uninstall` (32-bit), and `HKCU\...\Uninstall` using `READ_FLAGS = KEY_READ | KEY_WOW64_64KEY` (`src/programs.rs:12, 323-341`), calling `read_entry()` (`src/programs.rs:231-318`) to parse `UninstallString`, `DisplayName`, `WindowsInstaller`, `InstallLocation`, `InstallDate` (with `FILETIME` fallback via `filetime_to_date` / `civil_from_days` at `src/programs.rs:185-207`), `EstimatedSize`, and `DisplayIcon`, deduplicating by `(name, version, system_component)` (`src/programs.rs:331-335`) and assigning sequential `0..N` indices to `Program::id` (`src/programs.rs:343-345`).
  * The worker thread sends `Msg::Programs(list)` over an `mpsc::channel` (`src/main.rs:137`), spawns a nested thread to extract 32×32 RGBA icons via `system::icon_rgba` (`src/main.rs:143-152` → `src/system.rs:100-159`), and sequentially computes missing directory sizes via `programs::folder_size(path, 300_000)` (`src/main.rs:154-164` → `src/programs.rs:350-373`).
* **UI State & Modal Workflow (`src/main.rs:33-971`)**:
  * Every frame, `eframe::App::ui` (`src/main.rs:349-402`) drains `self.rx` in `handle_messages` (`src/main.rs:168-224`), handles `--screenshot` capture (`src/main.rs:354-377`), checks global keyboard shortcuts (`src/main.rs:380-389`), and renders `menu_bar` (`src/main.rs:406-445`), `toolbar` (`src/main.rs:447-507`), `status_bar` (`src/main.rs:509-530`), `details_panel` (`src/main.rs:532-597`), `program_list` (`src/main.rs:599-758`), and `dialogs` (`src/main.rs:760-970`).
  * Uninstalling transitions `App::dialog` through `Dialog::Confirm` → `Dialog::Running` (spawning a thread that invokes `system::shell_start` and waits on the Win32 `HANDLE` at `src/main.rs:271-287`) → either `Dialog::Waiting` or `Dialog::Scanning` (`src/main.rs:195-204`).
* **Leftover Scanning, Process Termination & Cleanup (`src/leftovers.rs:1-470`, `src/system.rs:161-246`)**:
  * `App::start_leftover_scan` (`src/main.rs:289-306`) spawns a thread running `leftovers::scan(&p, &others)` (`src/leftovers.rs:195-335`) and `system::find_processes_in_directory` (`src/system.rs:170-233`), transitioning to `Dialog::Leftovers` (`src/main.rs:205-214, 836-901`).
  * Confirming removal terminates detected processes via `system::terminate_process_by_pid` (`src/main.rs:891-893` → `src/system.rs:236-246`) and spawns a thread running `leftovers::remove` (`src/main.rs:959-968` → `src/leftovers.rs:351-383`), which recycles files/folders via `trash::delete` (`src/leftovers.rs:367`) and exports registry keys via `reg export` before calling `RegKey::delete_subkey_all` (`src/leftovers.rs:371-377`).

---

## 1. Critical Bugs & Edge Cases

### 1.1. Fallback `install_location` Resolution Can Resolve to `C:\Windows\System32` and Terminate Critical Windows System Processes
* **Location**: `src/programs.rs:253-272`, `src/main.rs:298-302`, `src/main.rs:860-873`, `src/main.rs:890-895`
* **Why it happens**:
  1. In `programs::read_entry` (`src/programs.rs:253-272`), when a registry entry has no `InstallLocation` value (or it is empty), Tabula Rasa falls back to extracting the parent directory of the executable in `UninstallString` (`src/programs.rs:254-259`) or `DisplayIcon` (`src/programs.rs:260-265`):
     ```rust
     if !is_msiexec(&exe) {
         derived = dir_of_existing_file(&exe);
     }
     ```
     `is_msiexec` (`src/programs.rs:171-173`) **only** checks for `msiexec.exe`. Many Windows drivers, INF packages, codecs, and system utilities use `C:\Windows\System32\rundll32.exe`, `cmd.exe`, `regsvr32.exe`, or `powershell.exe` as their `UninstallString` (or reference an icon in `C:\Windows\System32\shell32.dll` / `imageres.dll`). For all such entries, `dir_of_existing_file` returns **`C:\Windows\System32`** (or `C:\Windows`), and `p.install_location` is set to `C:\Windows\System32`.
  2. When `start_leftover_scan` runs (`src/main.rs:298-302`), it does **not** check `leftovers::is_safe_folder` before scanning for running processes:
     ```rust
     let procs = if !p.install_location.is_empty() {
         system::find_processes_in_directory(std::path::Path::new(&p.install_location))
     } else {
         Vec::new()
     };
     ```
     `find_processes_in_directory("C:\\Windows\\System32")` matches every running Windows system process (`svchost.exe`, `dwm.exe`, `explorer.exe`, `lsass.exe`, `winlogon.exe`, etc.).
  3. Worse still, in `Dialog::Leftovers` (`src/main.rs:890-895`), clicking **"Remove selected"** (even if the user only ticked a harmless registry leftover and **never** clicked `"Close running processes"`) unconditionally executes:
     ```rust
     for proc_ in running_procs.iter() {
         system::terminate_process_by_pid(proc_.pid);
     }
     ```
     This calls `TerminateProcess` on every matched process in `C:\Windows\System32`, immediately killing the user's desktop session or triggering a system crash.
* **Concrete, actionable solution**:
  1. In `src/programs.rs:253-272`, require any derived fallback directory `d` to pass `crate::leftovers::is_safe_folder(&d)` before assigning it to `install_location`, and expand `is_msiexec` into an `is_system_host_binary` check covering `msiexec`, `rundll32`, `cmd`, `powershell`, `pwsh`, `regsvr32`, `wscript`, `cscript`, and `conhost`.
  2. In `src/main.rs:298-302` and `src/main.rs:867-871`, only call `system::find_processes_in_directory` when `leftovers::is_safe_folder(std::path::Path::new(&p.install_location))` is `true` and the install folder is not shared with another installed program.
  3. In `src/main.rs:890-895`, remove the automatic `terminate_process_by_pid` loop from the `"Remove selected"` button handler so processes are only terminated when the user explicitly clicks `"Close running processes"`.

---

### 1.2. Case-Sensitive `Path::starts_with` on Windows Bypasses `is_safe_folder` and Shared-Folder Guardrails
* **Location**: `src/leftovers.rs:174-180`, `src/leftovers.rs:202-204`, `src/leftovers.rs:210-213`, `src/leftovers.rs:287`
* **Why it happens**:
  * Rust's `std::path::Path::starts_with` compares path components using `OsStr` equality, which is **case-sensitive** even on Windows.
  * In `leftovers::is_safe_folder` (`src/leftovers.rs:174-180`):
    ```rust
    for var in ["SystemRoot", "windir"] {
        if let Some(sys_root) = env_path(var) {
            if path.starts_with(&sys_root) {
                return false;
            }
        }
    }
    ```
    If `%SystemRoot%` is `C:\Windows` and a registry entry's `InstallLocation` is `c:\windows\System32\MySubdir` or `C:\WINDOWS\SysWOW64\Subdir`, `path.starts_with(&sys_root)` returns **`false`**, bypassing the Windows directory protection!
  * Similarly, in `leftovers::scan`:
    * `used_by_other` (`src/leftovers.rs:202-204`) calls `o.starts_with(p)`. If Program A is in `C:\Program Files\Vendor` and Program B is registered at `c:\program files\vendor\Plugin`, `o.starts_with(p)` returns **`false`**, so Program A's uninstallation offers the shared parent directory for deletion. Furthermore, `used_by_other` does not check `p.starts_with(o)` (when the uninstalled program resides in a subfolder of another still-installed application's root directory).
    * Folder deduplication (`src/leftovers.rs:210-213`) and shortcut deduplication (`src/leftovers.rs:287`) also rely on case-sensitive `starts_with`, allowing both a parent folder and its child folder/shortcut to be queued for deletion when path casing differs.
* **Concrete, actionable solution**:
  * Implement a case-insensitive path prefix helper in `src/leftovers.rs` and use it everywhere `starts_with` is called on `Path`:
    ```rust
    fn path_starts_with_ci(path: &Path, base: &Path) -> bool {
        let mut p_it = path.components();
        for b_comp in base.components() {
            let Some(p_comp) = p_it.next() else { return false };
            if !p_comp.as_os_str().to_string_lossy().eq_ignore_ascii_case(&b_comp.as_os_str().to_string_lossy()) {
                return false;
            }
        }
        true
    }
    ```
  * Replace `path.starts_with(&sys_root)` (`src/leftovers.rs:176`), `o.starts_with(p)` (`src/leftovers.rs:203`), `p.starts_with(&f.path)` (`src/leftovers.rs:210`), `Path::new(&f.path).starts_with(&p)` (`src/leftovers.rs:213`), and `lnk.starts_with(&f.path)` (`src/leftovers.rs:287`) with `path_starts_with_ci`.

---

### 1.3. Out-of-Bounds Index Panic and Stale `Program::id` Race Condition Across Refreshes
* **Location**: `src/main.rs:127-194`, `src/main.rs:380-382`, `src/main.rs:511`, `src/main.rs:739`, `src/main.rs:780-830`
* **Why it happens**:
  * `Program::id` is a positional index into `self.programs` (`0..out.len()`, assigned in `src/programs.rs:343-345`).
  * `App::refresh` (`src/main.rs:127-166`) sets `self.loading = true`, which is reset to `false` as soon as `Msg::Programs(list)` is received (`src/main.rs:174`). However, the background thread (`src/main.rs:154-164`) and its nested icon thread (`src/main.rs:143-152`) continue running asynchronously for several seconds, emitting `Msg::Size(id, s)` and `Msg::Icon(id, s, rgba)`.
  * In addition, `F5` (`src/main.rs:380-382`) can be pressed at any time—including while `Dialog::Confirm { id, .. }`, `Dialog::Running { id, .. }`, or `Dialog::Waiting { id, .. }` is open!
  * If a refresh completes while one of those dialogs is open and the newly scanned `self.programs` has fewer items (e.g., because the uninstaller just removed its registry key), direct slice indexing in `dialogs()`:
    * `let p = &programs[*id];` (`src/main.rs:781`)
    * `programs[*id].name` (`src/main.rs:808`)
    * `let p = &programs[*id];` (`src/main.rs:812`)
    will **panic with an index out of bounds crash** (or silently reference the wrong program if indices shifted).
  * Furthermore, in-flight `Msg::Size(id, s)` (`src/main.rs:181-186`) and `Msg::Icon(id, s, rgba)` (`src/main.rs:187-194`) from an earlier scan generation will overwrite the size and icon texture of whichever program now sits at index `id` in the new scan.
* **Concrete, actionable solution**:
  * Add a `scan_id: u64` generation counter to `App`, increment it on each `refresh()`, and include `scan_id` in `Msg::Programs`, `Msg::Size`, and `Msg::Icon`; ignore any message whose `scan_id != self.scan_id`.
  * Store the cloned `Program` struct (instead of `id: usize`) inside `Dialog::Confirm`, `Dialog::Running`, and `Dialog::Waiting` (just as `Dialog::Leftovers` already does at `src/main.rs:57`), and block `refresh()` while an uninstall modal is active.

---

### 1.4. Win32 Process `HANDLE` Leak in `system::shell_start` Callers
* **Location**: `src/system.rs:30-44`, `src/system.rs:48-87`, `src/system.rs:96`
* **Why it happens**:
  * `shell_start` (`src/system.rs:48-73`) always sets `info.fMask = SEE_MASK_NOCLOSEPROCESS` (`src/system.rs:55`) and wraps the returned `info.hProcess` in `pub struct Started(HANDLE)` (`src/system.rs:30, 70`).
  * `Started` does **not** implement `Drop`. `CloseHandle(self.0)` is only called inside `Started::wait(self)` (`src/system.rs:35-43`).
  * Four callers in `src/system.rs` invoke `shell_start` and immediately drop the returned `Result<Option<Started>, String>` without calling `.wait()`:
    * `open_path` (`src/system.rs:75-77`): `let _ = shell_start(path, "", Some("open"));`
    * `open_url` (`src/system.rs:79-81`): `let _ = shell_start(url, "", Some("open"));`
    * `relaunch_as_admin` (`src/system.rs:84-87`): `shell_start(&exe.to_string_lossy(), "", Some("runas")).is_ok()`
    * `open_in_regedit` (`src/system.rs:96`): `let _ = shell_start("regedit.exe", "-m", None);`
  * Every call to any of those functions leaks a kernel process `HANDLE` for the lifetime of the application.
* **Concrete, actionable solution**:
  * Implement `Drop` for `Started` in `src/system.rs:30-44` so `CloseHandle(self.0)` is always executed when `Started` is dropped, and update `Started::wait(&mut self)` (or `wait(self)`) so `Drop` handles closing the handle once:
    ```rust
    impl Drop for Started {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CloseHandle(self.0); }
            }
        }
    }
    ```

---

### 1.5. Unchecked Registry Backup Before Destructive `delete_subkey_all` and Missing 64-Bit View Flags
* **Location**: `src/leftovers.rs:370-379`
* **Why it happens**:
  * In `leftovers::remove` (`src/leftovers.rs:370-379`):
    ```rust
    let file = backup_dir.join(format!("{} {} {}.reg", safe_name.trim(), timestamp(), i));
    let _ = std::process::Command::new("reg")
        .args(["export", &shown, &file.to_string_lossy(), "/y"])
        .creation_flags_hidden()
        .status();
    let root = RegKey::predef(if item.machine { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER });
    root.delete_subkey_all(&item.path).map_err(|e| e.to_string())
    ```
  * First, the return status of `reg export` is completely ignored (`let _ = ...`). If `reg.exe` fails to create the `.reg` backup (e.g., due to permissions, invalid characters in `safe_name`, or disk error), `root.delete_subkey_all(&item.path)` still executes and permanently deletes the registry tree without a backup—contradicting the UI guarantee at `src/main.rs:886`.
  * Second, `leftovers::scan` opens registry keys using `READ_FLAGS = KEY_READ | KEY_WOW64_64KEY` (`src/leftovers.rs:308`), whereas `reg export` is invoked without `"/reg:64"` and `root.delete_subkey_all(&item.path)` (`src/leftovers.rs:377`) does not pass `KEY_WOW64_64KEY`.
* **Concrete, actionable solution**:
  * Pass `"/reg:64"` to `reg export`, check that `status()` returns `Ok(s) if s.success()` and that `file.exists()`, and return `Err("Failed to create registry backup (.reg); skipped deletion".into())` instead of calling `delete_subkey_all` when the export fails.

---

### 1.6. Icon Alpha-Reconstruction Artifacts, `.ico` Format Compatibility, and Silent Screenshot Export Failures
* **Location**: `src/system.rs:100-159`, `src/main.rs:354-377`, `build.rs:4-10`, `Tabula Rasa.ico`
* **Why it happens**:
  1. **Dual-blit icon alpha reconstruction (`src/system.rs:138-149`)**:
     * `icon_rgba` draws the extracted `HICON` onto a black buffer and a white buffer, then computes:
       ```rust
       let mut diff = 0i32;
       for c in 0..3 {
           diff = diff.max(on_white[o + c] as i32 - on_black[o + c] as i32);
       }
       let a = (255 - diff).clamp(0, 255);
       ```
       If a classic Win32 icon uses an inverted XOR mask (`AND=1, XOR=255`), drawing on black produces `255` and drawing on white produces `0`, so `on_white - on_black = -255`. Because `diff` starts at `0`, `diff.max(-255)` remains `0`, falsely computing `a = 255` (opaque black) instead of transparent/inverted.
     * In addition, `((on_black[o + src] as i32 * 255) / a).min(255)` (`src/system.rs:147`) truncates integer division without rounding (`+ a / 2`), systematically darkening semi-transparent anti-aliased edges.
     * Furthermore, `ExtractIconExW` (`src/system.rs:105`) can return `u32::MAX` (`UINT_MAX`) on error when inspecting a corrupt file; `n == 0 || large.is_null()` (`src/system.rs:106`) only survives because `large` remains `null_mut()`, not because `n` is checked for `u32::MAX`.
  2. **Silent PNG screenshot export failure (`src/main.rs:362-375`)**:
     * In `--screenshot` mode, both `let _ = std::fs::create_dir_all(parent);` (`src/main.rs:369`) and `let _ = buf.save(&path);` (`src/main.rs:372`) discard errors, set `done.store(true, Ordering::SeqCst)` (`src/main.rs:374`), and close the window with exit code `0` even if no PNG file was written.
  3. **PNG-encoded sub-256×256 frames in `Tabula Rasa.ico` (`build.rs:5`, `Tabula Rasa.ico`)**:
     * All 7 directory entries in `Tabula Rasa.ico` (`16×16`, `24×24`, `32×32`, `48×48`, `64×64`, `128×128`, `256×256`) contain raw PNG streams (`\x89PNG\r\n\x1a\n`). Standard Windows ICO format specifications recommend uncompressed `BITMAPINFOHEADER` DIBs for frames `< 256×256` and PNG only for `256×256`, as legacy Win32 `LoadImageW` / resource parsers can fail to render PNG-compressed small icon frames.
* **Concrete, actionable solution**:
  * In `src/system.rs:147`, round the un-premultiplied channel calculation: `((on_black[o + src] as i32 * 255 + (a / 2)) / a).min(255) as u8`, and check `n == 0 || n == u32::MAX || large.is_null()` at `src/system.rs:106`.
  * In `src/main.rs:368-375`, handle errors from `create_dir_all` and `buf.save(&path)` by printing to `eprintln!` and calling `std::process::exit(1)` on failure.
  * Re-encode `Tabula Rasa.ico` frames below `256×256` (`16×16` through `128×128`) as 32-bpp ARGB BMP/DIB payloads while keeping `256×256` as PNG.

---

### 1.7. Over-Greedy Parenthesis Stripping in `leftovers::name_stem`
* **Location**: `src/leftovers.rs:67-75`
* **Why it happens**:
  * `name_stem` strips parenthesized suffixes using `s.find('(')` paired with `s.rfind(')')`:
    ```rust
    while let (Some(a), Some(b)) = (s.find('('), s.rfind(')')) {
        if a < b {
            s.replace_range(a..=b, " ");
        } else {
            break;
        }
    }
    ```
  * Because `find('(')` locates the **first** opening parenthesis and `rfind(')')` locates the **last** closing parenthesis, any program name with two parenthesized groups—such as `"Foo (Bar) Studio (x64)"` or `"(Preview) App Name (64-bit)"`—has everything between the first `'('` and the last `')'` erased in a single iteration (turning `"(Preview) App Name (64-bit)"` into `""`!).
* **Concrete, actionable solution**:
  * Match each `'('` with the **next** closing `')'` after `a`:
    ```rust
    while let Some(a) = s.find('(') {
        if let Some(rel_b) = s[a..].find(')') {
            s.replace_range(a..=a + rel_b, " ");
        } else {
            break;
        }
    }
    ```

---

### 1.8. Canonicalized `\\?\` Extended-Length Path Prefix Breaks Process Matching on Symlinked Directories
* **Location**: `src/system.rs:173-176`, `src/system.rs:203-215`
* **Why it happens**:
  * In `find_processes_in_directory` (`src/system.rs:173-176`), `std::fs::canonicalize(dir)` on Windows returns an extended-length path prefixed with `\\?\` (e.g., `\\?\c:\program files\app`).
  * However, `QueryFullProcessImageNameW(h_proc, 0, ...)` (`src/system.rs:203`) returns a standard Win32 path without `\\?\` (e.g., `C:\Program Files\App\app.exe`).
  * Consequently, `p_lower.starts_with(&format!("{dir_canon}\\"))` (`src/system.rs:214`) **always evaluates to `false`**, rendering the canonicalization fallback completely dead whenever `dir` traverses a symlink, junction, or non-canonical path!
* **Concrete, actionable solution**:
  * Strip the `\\?\` prefix after canonicalizing in `src/system.rs:173-176`:
    ```rust
    let dir_canon = std::fs::canonicalize(dir)
        .ok()
        .map(|p| {
            let s = p.to_string_lossy().to_lowercase();
            s.strip_prefix(r"\\?\").unwrap_or(&s).trim_end_matches('\\').to_string()
        })
        .unwrap_or_default();
    ```

---

## 2. Architecture & Performance

### 2.1. Per-Frame $O(N \log N)$ String Allocations and Double Execution of `App::visible()`
* **Location**: `src/main.rs:226-253`, `src/main.rs:510`, `src/main.rs:600`, `src/main.rs:707-713`
* **Why it happens**:
  * `App::visible(&self)` (`src/main.rs:226-253`) filters and sorts the entire program list from scratch. It is called **twice every frame**: once in `status_bar()` (`src/main.rs:510`) and once in `program_list()` (`src/main.rs:600`).
  * Inside `visible()`, `.filter(...)` allocates lowercase `String`s (`p.name.to_lowercase()`, `p.publisher.to_lowercase()` at `src/main.rs:234-235`), and `.sort_by(...)` allocates up to **four new `String`s per comparison** (`a.name.to_lowercase().cmp(&b.name.to_lowercase())` at `src/main.rs:243, 244, 250`). For 300 installed programs (~2,500 comparisons × 2 calls per frame), this performs ~10,000+ temporary heap allocations per frame during mouse movement or scrolling.
  * Additionally, `details_panel()` (`src/main.rs:533`) clones the entire selected `Program` struct (12 `String` fields) every frame, and `program_list()` (`src/main.rs:707-713`) clones 5 `String`s for every visible row on every frame.
* **Concrete, actionable solution**:
  * Store pre-lowercased `name_lower: String` and `publisher_lower: String` fields on `Program` when constructed in `programs::read_entry` (`src/programs.rs:297-317`).
  * Cache `visible_ids: Vec<usize>` on `App` (or pass `&vis` from `ui()` into both `status_bar(ui, &vis)` and `program_list(ui, &vis)`), and pass borrowed `&str` slices in `cells: [(&str, f32, Color32, f32); 5]` (`src/main.rs:707`).

---

### 2.2. Per-Icon GDI Context Creation, Heap Allocations, and Runtime Image Downscaling
* **Location**: `src/main.rs:92-100`, `src/main.rs:142-152`, `src/main.rs:458`, `src/main.rs:697-702`, `src/system.rs:100-159`
* **Why it happens**:
  * For every program in `icon_list` (`src/main.rs:143-152`), `system::icon_rgba` (`src/system.rs:109-156`) acquires the screen DC (`GetDC`), creates a memory DC (`CreateCompatibleDC`), allocates a new 32×32 `DIBSection` (`CreateDIBSection`), allocates three `Vec<u8>` buffers of 4,096 bytes each (`on_black`, `on_white`, `rgba` at `src/system.rs:129, 133, 135`), and destroys the GDI objects—repeating this setup/teardown hundreds of times in a tight loop and calling `ctx2.request_repaint()` after every single icon (`src/main.rs:148`).
  * In `src/main.rs:92-100`, `assets/icon-64.png` (64×64) is loaded and downscaled every frame to 36×36 (`src/main.rs:458`), and 32×32 program icons are downscaled every frame to 20×20 in the table (`src/main.rs:697`) without mipmaps (`TextureOptions::LINEAR`), causing subtle aliasing shimmer on 1.0× DPI displays.
* **Concrete, actionable solution**:
  * Create a reusable `IconExtractor` struct in `src/system.rs` that holds one `HDC`, one 32×32 `DIBSection`, and two `[u8; 4096]` stack/struct scratch buffers for the duration of the icon extraction thread, and batch `ctx2.request_repaint()` (e.g., every 16 icons or at thread completion).

---

### 2.3. Repeated Environment Variable & Filesystem `stat` Probing in `protected_folders()`
* **Location**: `src/leftovers.rs:113-191`, `src/main.rs:156`
* **Why it happens**:
  * `is_safe_folder(path)` (`src/leftovers.rs:156-191`) calls `protected_folders()` (`src/leftovers.rs:122-154`) on **every invocation**, which reads 15+ OS environment variables and executes 8 filesystem `.is_dir()` metadata queries via `env_path()` (`src/leftovers.rs:113-115`), plus 3 more `.is_dir()` queries in `is_safe_folder` (`src/leftovers.rs:175, 182`).
  * During `App::refresh` (`src/main.rs:156`) and `leftovers::scan` (`src/leftovers.rs:207`), `is_safe_folder` is called dozens of times with unchanging environment paths.
* **Concrete, actionable solution**:
  * Cache the protected folder list once using `std::sync::LazyLock<Vec<PathBuf>>` in `src/leftovers.rs`.

---

### 2.4. Synchronous Filesystem `is_dir()` and Process Termination Calls on the UI Render Thread
* **Location**: `src/main.rs:477`, `src/main.rs:742`, `src/main.rs:864-871`, `src/main.rs:891-893`
* **Why it happens**:
  * `toolbar()` (`src/main.rs:477`) runs `std::path::Path::new(l).is_dir()` on **every single UI frame** whenever a program is selected. If `install_location` points to a spun-down external HDD or an unreachable network/UNC share, `is_dir()` blocks the main UI thread for seconds on every frame.
  * Similarly, clicking `"Close running processes"` (`src/main.rs:860-872`) runs `terminate_process_by_pid` and a full system `CreateToolhelp32Snapshot` + `OpenProcess` + `QueryFullProcessImageNameW` scan synchronously on the UI thread, freezing the window.
* **Concrete, actionable solution**:
  * Cache `install_dir_exists: bool` on `Program` during background scanning (`src/programs.rs:253`), and move process termination + re-scanning onto a background thread communicating via `Msg`.

---

### 2.5. ASCII-Lexicographical Version Sorting and Unvalidated Date Normalization
* **Location**: `src/main.rs:247`, `src/programs.rs:209-229`
* **Why it happens**:
  * `SortCol::Version` (`src/main.rs:247`) compares raw version strings lexicographically (`a.version.cmp(&b.version)`), sorting `"10.2.0"` before `"2.1.0"`.
  * `normalize_date` (`src/programs.rs:209-229`) accepts any 8 digits starting with `"19"` or `"20"` without checking valid month (`1..=12`) or day (`1..=31`) ranges, and parses `m/d/yyyy` (`src/programs.rs:218-227`) without bounds checks (allowing e.g. `99/99/2024` → `"2024-99-99"`).
* **Concrete, actionable solution**:
  * Compare version strings using a natural/numeric comparator that parses contiguous digit runs as `u64`, and validate `(1970..=2100).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d)` in `normalize_date`.

---

## 3. UI/UX & Accessibility Issues

### 3.1. `Delete` Key Triggers Uninstall Modal While Editing the Search Filter Input
* **Location**: `src/main.rs:383-389`, `src/main.rs:656`
* **Why it happens**:
  * At `src/main.rs:383-389`, the global `Key::Delete` handler checks `if matches!(self.dialog, Dialog::None)` and `if let Some(id) = self.selected`, but **omits `!ctx.egui_wants_keyboard_input()`** (which was properly included for `ArrowUp`/`ArrowDown`/`Enter` at `src/main.rs:656`):
    ```rust
    if matches!(self.dialog, Dialog::None) {
        if let Some(id) = self.selected {
            if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
                self.ask_uninstall(id);
            }
        }
    }
    ```
  * If a user selects a program, presses `Ctrl+F` to focus the search box (`src/main.rs:491-493`), and presses `Delete` to delete characters in their search query, Tabula Rasa intercepts `Delete` and pops up the **"Uninstall program"** confirmation modal!
* **Concrete, actionable solution**:
  * Guard `src/main.rs:383` with `if matches!(self.dialog, Dialog::None) && !ctx.egui_wants_keyboard_input()`.

---

### 3.2. Table Column Overflow, Right-Edge Clipping, and Toolbar Collision at Minimum Width / High Zoom
* **Location**: `src/main.rs:456-495`, `src/main.rs:601-639`, `src/main.rs:687`, `src/main.rs:991`
* **Why it happens**:
  * In `program_list()` (`src/main.rs:601-606`), column widths are computed as:
    ```rust
    let full_w = ui.available_width();
    let size_w = 90.0;
    let date_w = 100.0;
    let ver_w = 120.0;
    let pub_w = (full_w * 0.24).clamp(140.0, 300.0);
    let name_w = (full_w - size_w - date_w - ver_w - pub_w - 16.0).max(160.0);
    ```
    When `full_w` is reduced (e.g., when DPI scaling or `egui` zoom is > 1.0, or when `name_w` hits its `.max(160.0)` floor below `626.0` logical pixels), `name_w + pub_w + size_w + date_w + ver_w` exceeds `full_w`, clipping the `Version` column off the right edge of the window.
  * In addition, `full_w` is captured **before** `ScrollArea::vertical()` (`src/main.rs:601`), and rows inside `show_rows` allocate `Vec2::new(full_w, ROW_H)` (`src/main.rs:687`), causing row content on the right edge to extend underneath the vertical scrollbar.
  * In `toolbar()` (`src/main.rs:456-495`), the brand header, three action buttons, and a `240.0`-wide search input are placed in a single non-wrapping `ui.horizontal`, clipping the search bar on narrow logical widths.
* **Concrete, actionable solution**:
  * Reserve scrollbar width (`ui.spacing().scroll.allocated_width()`) when computing column widths, scale `pub_w`, `size_w`, `date_w`, and `ver_w` proportionally so their total never exceeds `available_width`, and replace `.desired_width(240.0)` in `toolbar()` (`src/main.rs:487`) with `.desired_width((ui.available_width() - 8.0).clamp(120.0, 240.0))`.

---

### 3.3. Mid-Character Text Slicing Without Ellipsis or Tooltip in Program List Cells
* **Location**: `src/main.rs:714-721`
* **Why it happens**:
  * Table cells lay out text with `f32::INFINITY` wrap width and hard-clip the painter rectangle:
    ```rust
    let cell = egui::Rect::from_min_size(egui::pos2(x + pad, rect.top()), Vec2::new(w - pad - 6.0, ROW_H));
    let galley = ui.painter().layout(txt, egui::FontId::proportional(13.5), col, f32::INFINITY);
    let clip = ui.painter_at(cell);
    clip.galley(egui::pos2(cell.left(), cell.center().y - galley.size().y / 2.0), galley, col);
    ```
  * Long program names, publishers, and version numbers are sliced abruptly mid-glyph at `cell.right()` without an ellipsis (`…`) and without an `.on_hover_text(...)` tooltip.
* **Concrete, actionable solution**:
  * Use `egui::text::LayoutJob::single_section` configured with `TextWrapping { max_width: cell.width(), max_rows: 1, break_anywhere: true, overflow_character: Some('…') }`, and attach `.on_hover_text(&p.name)` to `resp` when text is elided.

---

### 3.4. Jarring Scroll-to-Center Jumps on Arrow Keys, Missing `Home`/`End`/`PageUp`/`PageDown`, and Missing `Esc` / Cancel in Modals
* **Location**: `src/main.rs:656-680`, `src/main.rs:803-830`, `src/main.rs:845-900`
* **Why it happens**:
  1. **Forced scroll re-centering (`src/main.rs:673-680`)**: Pressing `ArrowDown` or `ArrowUp` sets `self.scroll_to_selected = true`, which unconditionally calls `scroll.vertical_scroll_offset((p as f32 * ROW_H - viewport_h / 2.0).max(0.0))` (`src/main.rs:676-677`). Even if the selected row is the 2nd item from the top and already visible, pressing `ArrowDown` jumps the scroll position. `Home`, `End`, `PageUp`, and `PageDown` are not handled at all.
  2. **Missing `Esc` support in `Dialog::Waiting` and `Dialog::Leftovers` (`src/main.rs:811-830`, `src/main.rs:845-900`)**: Although `README.md:51` states that `Esc` closes the active modal dialog, `close_on_escape = true` is **missing** in `Dialog::Waiting` (`src/main.rs:811-830`) and in the non-empty branch of `Dialog::Leftovers` (`src/main.rs:845-900`).
  3. **No escape hatch in `Dialog::Running` (`src/main.rs:803-810`)**: While waiting on `proc_.wait()` (`src/main.rs:280`), `Dialog::Running` renders no `"Stop waiting"` or `"Cancel"` button. If an uninstaller spawns a background helper process that inherits the handle and never exits, the UI is permanently locked in `Dialog::Running`.
* **Concrete, actionable solution**:
  * Set `close_on_escape = true` in `Dialog::Waiting` (`src/main.rs:829`) and the `else` branch of `Dialog::Leftovers` (`src/main.rs:900`).
  * Add a `"Stop waiting"` button to `Dialog::Running` (`src/main.rs:803-810`) that transitions `self.dialog` to `Dialog::Waiting { id: *id, exit: Ok(0) }`.
  * Add `Home`, `End`, `PageUp`, and `PageDown` key handling in `src/main.rs:656-670`, and use `ui.scroll_to_rect(rect, None)` on the selected row inside `show_rows` instead of overriding `vertical_scroll_offset`.

---

## 4. Code Quality & Cleanup

### 4.1. Dead Warning-Suppression Statement `let _ = Kind::Folder;`
* **Location**: `src/main.rs:15`, `src/main.rs:969`
* **Why it happens**:
  * `src/main.rs:15` imports `use leftovers::{Kind, Leftover};`, even though `main.rs` never constructs or matches on `Kind` directly (it only calls `it.kind.label()` at `src/main.rs:879`). To suppress the resulting compiler warning on `Kind`, `let _ = Kind::Folder;` was placed at the end of `App::dialogs` (`src/main.rs:969`).
* **Concrete, actionable solution**:
  * Remove `let _ = Kind::Folder;` from `src/main.rs:969` and change `src/main.rs:15` to `use leftovers::Leftover;`.

---

### 4.2. Redundant Registry Queries in `programs::read_entry` and Duplicated Path-Equality Helpers
* **Location**: `src/programs.rs:233-295`, `src/main.rs:264`, `src/leftovers.rs:117-119`, `src/system.rs:172`
* **Why it happens**:
  * In `programs::read_entry` (`src/programs.rs:233-295`), `get_str(&key, "UninstallString")` is queried **three times** (`lines 233, 254, 292`), `split_command` and `expand_env` are called **twice** on the exact same string (`lines 254-255` and `lines 292-293`), and `parse_icon(&get_str(&key, "DisplayIcon"))` is called **twice** (`lines 261` and `291`), performing duplicate file-existence syscalls (`Path::new(&path).is_file()` inside `parse_icon` at `src/programs.rs:138`) for every installed program.
  * Case-insensitive path comparison (`trim_end_matches('\\').eq_ignore_ascii_case(...)`) is re-implemented inline in `src/main.rs:264`, `src/leftovers.rs:117-119`, and `src/system.rs:172, 210-214`.
* **Concrete, actionable solution**:
  * In `programs::read_entry`, compute `(uninstall_exe, _)` and `parsed_icon` once at the top of the function and reuse them for both `install_location` fallback (`src/programs.rs:253-272`) and `icon` fallback (`src/programs.rs:291-295`).
  * Export `pub fn same_path(a: &Path, b: &Path) -> bool` from `src/leftovers.rs:117` and call it in `src/main.rs:264`.

---

### 4.3. Unchecked Win32 Wait / Exit-Code Calls and Deprecated `IsUserAnAdmin`
* **Location**: `src/system.rs:26-28`, `src/system.rs:35-43`
* **Why it happens**:
  * `Started::wait` (`src/system.rs:35-43`) ignores the return values of both `WaitForSingleObject(self.0, INFINITE)` and `GetExitCodeProcess(self.0, &mut code)`, returning `0` (success) if either Win32 call fails.
  * `is_admin()` (`src/system.rs:26-28`) calls `IsUserAnAdmin()`, which Microsoft marks as a deprecated wrapper around `OpenProcessToken` / `GetTokenInformation(TokenElevation)`.
* **Concrete, actionable solution**:
  * Check `WaitForSingleObject(self.0, INFINITE) == WAIT_OBJECT_0` and `GetExitCodeProcess(self.0, &mut code) != 0` in `Started::wait` (`src/system.rs:35-43`), returning `Result<u32, String>` so wait failures are reported accurately to `Msg::UninstallDone`.

---

### 4.4. Tracked Reverse-Engineering / Build-Harness Artifacts in Repository Root
* **Location**: `.gitignore:1-11`, `capture/README.md`, `investigation.cany/`, `build-activity.log`, `agent-session.jsonl`, `craft.json`, `feed.json`, `usage.json`, `inbox/`, `pinned/`, `checks/`, `shots/`
* **Why it happens**:
  * Although `.gitignore` (`lines 1-10`) excludes `*.log`, `agent-session.jsonl`, `craft.json`, `feed.json`, `usage.json`, `inbox/`, `pinned/`, `checks/`, and `/shots`, several of these files (`capture/`, `investigation.cany/`, `mechanics.json`, `reference/picture-1.png`) are not listed in `.gitignore`, and the ignored harness files remain present in the root workspace alongside the Rust crate.
* **Concrete, actionable solution**:
  * Add `/capture/` and `/investigation.cany/` to `.gitignore` (`lines 1-11`) and clean up harness session logs if publishing the repository as a standalone crate.
