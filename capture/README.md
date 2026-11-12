# What we found in C:\Program Files (x86)\HiBit Uninstaller\HiBitUninstaller.exe

HiBitUninstaller.exe is a program for Windows, built for x86 (32-bit). It looks like it was made with Delphi or C++Builder. Its internal names were removed (stripped), so most functions will have generated names.

Evidence ev_077fa7ca3807421d9aad600f867290a085d4e6f037225bedfc9d56abc9859895.

```json
{
  "architecture": "x86",
  "bits": 32,
  "dotnet_arch": null,
  "entry_point": "0xb20e10",
  "format": "pe",
  "format_name": "PE (Windows)",
  "kind": "executable",
  "libraries": [
    "WTSAPI32.DLL",
    "advapi32.dll",
    "comctl32.dll",
    "comdlg32.dll",
    "dnsapi.dll",
    "gdi32.dll",
    "kernel32.dll",
    "msvcrt.dll",
    "ole32.dll",
    "oleacc.dll",
    "oleaut32.dll",
    "shell32.dll",
    "user32.dll",
    "version.dll",
    "wininet.dll",
    "winmm.dll",
    "winspool.drv",
    "wintrust.dll",
    "ws2_32.dll",
    "wsock32.dll",
    "msimg32.dll",
    "rpcrt4.dll",
    "windowscodecs.dll",
    "uxtheme.dll",
    "imm32.dll",
    "DWMAPI.DLL",
    "Shcore.dll"
  ],
  "next_steps": [
    {
      "tool": "core_overview_show",
      "why": "a one-page summary: sections, imports, counts"
    },
    {
      "tool": "native_functions_list",
      "why": "every function we found"
    },
    {
      "tool": "native_strings_find",
      "why": "find text the program shows or uses"
    },
    {
      "tool": "native_function_study",
      "why": "a full dossier on one function (start with main or the entry point)"
    },
    {
      "tool": "core_feature_find",
      "why": "follow a word you see in the app to the code behind it"
    },
    {
      "tool": "native_pe_resources_list",
      "why": "icons, version information and the manifest"
    }
  ],
  "operating_system": "Windows",
  "plain_english": "HiBitUninstaller.exe is a program for Windows, built for x86 (32-bit). It looks like it was made with Delphi or C++Builder. Its internal names were removed (stripped), so most functions will have generated names.",
  "recommended_toolsets": [
    "core",
    "native",
    "formats"
  ],
  "stripped": true,
  "symbol_table_stripped": true,
  "toolchain_hints": [
    {
      "category": "language",
      "evidence": "an .itext section",
      "id": "delphi",
      "label": "Delphi or C++Builder",
      "strength": "strong"
    }
  ]
}
```

## Ghidra

Deep mode: Ghidra has analysed C:\Program Files (x86)\HiBit Uninstaller\HiBitUninstaller.exe (3 min), so decompiling it is quick.
