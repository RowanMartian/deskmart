# Notice

Deskmart is a derivative work of **Pearcleaner** by alienator88
(https://github.com/alienator88/Pearcleaner), licensed under the Apache License 2.0 with the
Commons Clause License Condition v1.0. The full license is in `LICENSE.md` and applies to Deskmart too:
you may use, change and share it, but you may not sell it or a product whose value comes substantially from it.

Deskmart is not affiliated with or endorsed by the Pearcleaner author.

## What changed from Pearcleaner

- Rewritten from Swift/SwiftUI to Rust. The interface is HTML/CSS/JS shown in the Mac's built-in WebKit
  (via tao + wry), styled after Dropfin.
- Built without Xcode project files: the app bundle is assembled by `scripts/bundle-macos.sh` on GitHub's Mac runners.
- The search logic (locations, per-app conditions, skip lists, Strict/Enhanced/Deep matching, orphan search,
  Lipo, translation pruning, developer paths) is ported from Pearcleaner's `Locations.swift`, `Conditions.swift`,
  `AppPathsFetch.swift`, `ReversePathsFetch.swift`, `Lipo.swift`, `Logic.swift` and `DevelopmentView.swift`.
- Updated for macOS 27 Golden Gate: the Full Disk Access check no longer reads TCC.db (blocked on macOS 27),
  folders macOS refuses to open are reported instead of silently skipped, and new system folders are skipped.
- Administrator actions use the standard macOS password prompt instead of a privileged helper
  (which requires a paid Apple Developer ID).
- Code-signature entitlements and Team IDs are read directly from Mach-O files instead of the Security framework.
- Not ported: the Finder extension (needs an Xcode app extension), the privileged helper, the iOS app installer,
  and the TCC permission viewer (macOS 27 no longer lets apps read the TCC database).

## Third-party assets

- Font Awesome Free 7 (icons CC BY 4.0, font SIL OFL 1.1, code MIT), bundled in `web/fa/`.
