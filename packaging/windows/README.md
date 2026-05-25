# Windows packaging (scaffold)

This directory holds the Windows installer recipe for Lamprey MAI.

**Status:** scaffold only, NOT built or signed yet.
PACKAGING-01 (deferred) will harden this — code-signing certificate,
authenticode timestamping, MSI variant, fresh-machine build verification.

## Contents

- `lamprey-mai.iss` — Inno Setup 6 script. Bundles the three shipped
  exes (`lamprey-mai`, `lamprey-mai-api`, `lamprey-mai-admin`) and the
  visual assets (startup image, install-screen splash, ASCII banner).
- The wizard splash + small icon both point at
  `docs/assets/lamprey-mai-install-screen.png` — the gold "LAMPREY MAI"
  badge.

## Building locally

Pre-reqs: [Inno Setup 6](https://jrsoftware.org/isinfo.php) on `PATH`,
and a release build of the three exes available under
`packaging/windows/bin/`:

```powershell
cd mai
cargo build --release -p lamprey-mai -p lamprey-mai-api -p lamprey-mai-admin
mkdir packaging\windows\bin -Force | Out-Null
Copy-Item target\release\lamprey-mai*.exe packaging\windows\bin\
ISCC.exe packaging\windows\lamprey-mai.iss
```

The installer drops as
`packaging/windows/Output/lamprey-mai-setup-<version>.exe`.

## Visual assets

All canonical assets live under `docs/assets/`. Each surface picks
the appropriate one:

| Surface                                  | Asset                                            |
| ---------------------------------------- | ------------------------------------------------ |
| Inno Setup wizard splash                 | `lamprey-mai-install-screen.png` (gold badge)    |
| Inno Setup `lamprey-mai-setup.exe` icon  | `lamprey-mai.ico`                                |
| `lamprey-mai.exe` Explorer/taskbar icon  | `lamprey-mai.ico` (embedded via `embed-resource`)|
| Start Menu + Desktop shortcut icon       | `lamprey-mai.ico` (`IconFilename=`)              |
| Add/Remove Programs uninstall entry icon | `lamprey-mai.exe` (resource pulled from the exe) |
| `lamprey-mai.exe` startup splash         | `lamprey-startup-image.png` (silhouette)         |
| Terminal banner after splash             | `lamprey-banner.txt` (ASCII)                     |

Notes:

- `lamprey-mai-icon.png` is the transparent-background source PNG
  (corner-floodfill removed the black backdrop). The `.ico` is
  generated from it via a one-shot Python script; not part of the
  build.
- The launcher's startup-splash PNG and the ASCII banner are baked
  into the exe via `include_bytes!` (see
  `tools/mai-launcher/src/splash.rs` and `src/main.rs`). Shipping
  them under `{app}\assets\` at install time is for operator
  inspection, not runtime use.
- The Explorer/taskbar icon is compiled into the .exe at build time
  via `tools/mai-launcher/build.rs` +
  `tools/mai-launcher/lamprey-mai.rc`. Shipping the `.ico` to
  `{app}\assets\` lets the shortcut entries pin their icon to that
  file directly, decoupling shortcut appearance from the build
  toolchain.
