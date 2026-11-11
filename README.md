# Deskmart

A native Mac app cleaner. Uninstall apps together with everything they leave behind, find files from apps you already deleted, and clear out developer caches.
Deskmart is a Rust rebuild of [Pearcleaner](https://github.com/alienator88/Pearcleaner) by alienator88, which is on hold and doesn't support macOS 27 Golden Gate. Deskmart does.

- **Apps:** see every file an app keeps (caches, preferences, containers, launch agents, logs) and move the app and its files to the Trash in one go
- **Orphans:** find files left behind by apps that are already gone
- **Developer:** clear Xcode DerivedData, simulators, npm, Cargo, Gradle, pip, CocoaPods and other caches, plus stale VS Code and Cursor workspace storage
- **File Search:** find files by name, extension, size, date, kind, tags or Finder comment
- **Lipo:** slim universal apps down to just the code your Mac runs, without Xcode
- **Updater:** find updates from Sparkle feeds, the Mac App Store and Homebrew, and install Sparkle updates after checking their signature
- **Homebrew:** installed packages, search and install, updates, cleanup, doctor and taps
- **Packages, Plugins, Services:** installer receipts, audio units and other plugins, launch agents and daemons
- **Restore:** everything goes to the Trash first, and you can put it back
- Drop an app on the window or the Dock icon, use `deskmart://uninstall?path=…` links, or the `deskmart` Terminal command
- A small, fast native app. Written in Rust, using the Mac's built-in WebKit for the interface.

---

## Getting the app: GitHub builds it for you

A Mac app has to be compiled on a Mac. GitHub provides free Mac build machines, so you don't need Xcode or anything else installed.
You only do this once. After that, every change rebuilds the app automatically.

### 1. Put this project on GitHub

1. Sign in at **github.com**. A free account works.
2. Click **+** (top right) → **New repository**.
3. Name it `deskmart`. Public is best (see Updates below), but **Private** works too. Don't tick "Add a README". Click **Create repository**.
4. Upload the files using one of these options:
   - **Easiest:** tell Claude the repository name (for example `your-name/deskmart`) and it pushes the code for you.
   - **GitHub Desktop:** File → Add Local Repository → choose this folder → Publish.
   - **Terminal**, from inside this folder:
     ```bash
     git remote add origin https://github.com/YOUR-NAME/deskmart.git
     ```
     ```bash
     git push -u origin main
     ```

### 2. Let GitHub build it (about 10 minutes)

Open the repository's **Actions** tab. You'll see **Build Mac app** running. When it shows a green tick, it's done.
If GitHub asks you to enable Actions for the repository, click the button to allow them.

### 3. Download and install

1. Go to the repository's main page → **Releases** (right-hand side) → **Deskmart — latest build**.
2. Download **Deskmart.dmg**, open it, and drag **Deskmart** onto **Applications**.
3. The first time you open it, macOS warns that it can't verify the developer. Deskmart isn't from the App Store or notarized by Apple.
   Go to **System Settings → Privacy & Security**, scroll down, and click **Open Anyway**.
   Or run this once in Terminal:
   ```bash
   xattr -cr /Applications/Deskmart.app
   ```
4. Give Deskmart **Full Disk Access** (see below). Deskmart shows you how the first time it opens.

---

## Permissions

| Permission | Why | Where |
| --- | --- | --- |
| Full Disk Access | To look in every Library folder, including other apps' containers | System Settings → Privacy & Security → Full Disk Access |
| App Management | To move other apps to the Trash and slim them with Lipo | System Settings → Privacy & Security → App Management |
| Your Mac password | Only for files owned by the system, such as in `/Library`. Uses the standard macOS prompt; Deskmart never sees it | Asked when needed |

In System Settings, click **+**, choose Deskmart in Applications, and switch it on.

After you install a new Deskmart build, you may need to switch these off and on again: the GitHub build is signed without
an Apple developer certificate, so macOS can treat each build as a new app. [Keep permissions across updates](#keep-permissions-across-updates-optional-one-time) fixes this.
If you just turned on Full Disk Access, quit and reopen Deskmart.

### macOS 27 Golden Gate

- macOS 27 keeps apps' data private from other apps. Without Full Disk Access, Deskmart can't see inside other apps' containers.
  Instead of quietly missing those files, Deskmart tells you which folders it couldn't read.
- Pearcleaner checked for Full Disk Access by reading the TCC database, which macOS 27 no longer allows. Deskmart checks a different way, so the status is right.
- Golden Gate runs only on Apple silicon. Lipo shows how much space the unused Intel code takes, and the app list marks Intel-only apps, which need Rosetta.
- Deskmart also runs on macOS 11 Big Sur and later, on Apple silicon and Intel.

---

## Using it

Open **Apps** and click an app. Deskmart lists the app and every file it found for it, already ticked. Untick anything you want to keep, then click **Uninstall**.
If you untick the app itself, the button becomes **Remove Items**, which cleans up leftovers and keeps the app.

Everything goes to the Trash, inside a folder named after the app and the time, for example `Slack_2026-11-11_09-30-12`.
Click **Undo** on the message that appears, or open **… → Restore Deleted Files** to put things back. The last 10 removals are kept.

### Search sensitivity

Settings → General has three levels, the same as Pearcleaner:

| Level | Finds |
| --- | --- |
| **Strict** (default) | Exact matches for the app name, bundle ID and entitlements |
| **Enhanced** | Also partial matches. May find a few unrelated files |
| **Deep** | Also the developer's company name and Team ID, Finder comments and files the app created. Finds the most, including files from the developer's other apps |

You can set a different level for one app from its **Options** menu.

### Sentinel

Settings → General → **Sentinel** watches the Trash. When you drag an app to the Trash yourself, Deskmart finds its leftover files (Strict level) and moves them to the Trash too.
It runs as a small background agent (`~/Library/LaunchAgents/app.deskmart.sentinel.plist`).

### Terminal

Settings → General → **Terminal command** adds `deskmart` to `/usr/local/bin`.

```bash
deskmart list
```
```bash
deskmart search "Visual Studio Code"
```
```bash
deskmart uninstall /Applications/Slack.app --level enhanced
```
```bash
deskmart orphans
```

Run `deskmart help` for everything. Terminal needs Full Disk Access too for complete results.

### Links

`deskmart://uninstall?path=/Applications/Foo.app` opens Deskmart and starts uninstalling that app (you still confirm).
`deskmart://open?path=/Applications/Foo.app` just shows its files. Useful in Shortcuts and Raycast or Alfred.

### Keyboard

| | |
| --- | --- |
| ⌘1 … ⌘0 | Apps, Orphans, Developer, File Search, Homebrew, Lipo, Packages, Plugins, Services, Updater |
| ⌘F | Search |
| ⌘R | Refresh or scan again |
| ⌘⌫ | Uninstall the open app |
| ⌥⌘Z | Restore Deleted Files |
| ⌘, | Settings |
| ⇧⌘C | Console |
| Esc | Back to the app list |
| ⌘-click, ⇧-click | Select several apps to uninstall together |

### Updates

Every time GitHub builds a new version, copies of Deskmart built by GitHub notice it on their own.
A small dot appears next to **…** at the bottom of the sidebar, and that menu gains **Update Available…**.
Click **Update and Relaunch**: Deskmart downloads the new build, checks it matches what GitHub published,
moves the current copy to the Trash (you can restore it from **Restore Deleted Files**), installs the new one and reopens. Your settings stay.

The check is one small request to GitHub, at most a few times a day. It only works if the repository is **public**,
because GitHub doesn't show private repositories' releases without signing in. With a private repository, the check quietly does nothing.

### Keep permissions across updates (optional, one-time)

macOS remembers Full Disk Access per app signature. Without an Apple developer account, each GitHub build is signed differently,
so after an update you may need to switch Full Disk Access off and on again. To avoid that, give the build a certificate of its own, once:

1. On your Mac, in Terminal, from this project's folder:
   ```bash
   bash scripts/make-signing-cert.sh
   ```
2. It copies the certificate to your clipboard and prints a password. On GitHub, open the repository → **Settings → Secrets and variables → Actions → New repository secret**,
   and add `DESKMART_SIGNING_P12` (paste) and `DESKMART_SIGNING_PASSWORD` (the printed password).
3. In the **Actions** tab, run **Build Mac app** again. Install that build by hand once, and turn on Full Disk Access for it.

From then on, every build is signed by the same certificate, permissions survive updates, and the in-app updater refuses any build that isn't signed by it.

### Good to know

- Deskmart never deletes anything outright. It only moves things to the Trash. Empty the Trash when you're sure.
- Some locations are always protected, whatever a search finds: your home folder, `/Applications`, `/Library` and the standard Library folders themselves, and anything in the system volume.
- The Updater sends your apps' bundle IDs to Apple (App Store lookups) and contacts each app's update feed, only when you check.
- The interface is served only to the app itself, on `localhost`, with a per-launch key. Other devices on your network can't reach it.
- Settings live in `~/Library/Application Support/Deskmart`. To uninstall Deskmart, turn off Sentinel and the Terminal command, then delete the app and that folder. Or use Deskmart on itself from another copy.

---

## Differences from Pearcleaner

| Pearcleaner | Deskmart |
| --- | --- |
| Swift + SwiftUI, built with Xcode | Rust, interface in the Mac's WebKit, built by GitHub Actions without an Xcode project |
| Privileged helper for system files | The standard macOS password prompt |
| Finder extension | **Open With → Deskmart**, drop on the Dock icon, or `deskmart://` links |
| Full Disk Access check reads TCC.db (broken on macOS 27) | Checks protected folders directly; reports folders it couldn't read |
| TCC permission viewer per app | Not included (macOS 27 blocks reading the TCC database) |
| Installs iOS apps, Sparkle updates in-app | Installs Sparkle updates; App Store updates open in the App Store |

---

## For developers

| Path | What it is |
| --- | --- |
| `src/apps.rs` | Finding apps and reading their bundle info, casks, dates and sizes |
| `src/finder.rs` | Leftover-file search (Strict/Enhanced/Deep) and orphan search |
| `src/locations.rs` | Search locations, per-app conditions, skip lists |
| `src/macho.rs` | Mach-O: architectures, entitlements and Team ID, Lipo thinning |
| `src/trash.rs` | Moving to the Trash, admin fallback, restore history |
| `src/tools.rs` | Translations, developer caches, packages, plugins, services, permission check |
| `src/filesearch.rs`, `src/brew.rs`, `src/updater.rs` | File Search, Homebrew, Updater |
| `src/sentinel.rs` | The Trash-watching background agent |
| `src/selfupdate.rs` | Updating Deskmart itself from the latest GitHub build |
| `scripts/make-signing-cert.sh` | One-time: a certificate so permissions survive updates |
| `src/api.rs` | Local HTTP API used by the interface |
| `src/gui.rs` | Native window (tao + wry + muda) |
| `src/cli.rs` | The `deskmart` Terminal command |
| `web/index.html` | The whole interface (HTML/CSS/JS, embedded in the binary) |
| `scripts/bundle-macos.sh` | Builds the universal `.app`, `.dmg` and `.zip` on a Mac |

Run the app:

```bash
cargo run
```

Run only the helper, then open `http://localhost:47311` in a browser (no window, useful for testing; works on Linux too):

```bash
cargo run --no-default-features -- --server-only --port 47311
```

Run the tests:

```bash
cargo test --no-default-features
```

## License and credits

Deskmart is based on [Pearcleaner](https://github.com/alienator88/Pearcleaner) by alienator88 and keeps its license:
**Apache 2.0 with the Commons Clause**. You can use, change and share it, but not sell it. See `LICENSE.md` and `NOTICE.md`.
Deskmart isn't affiliated with or endorsed by the Pearcleaner author.

Icons: [Font Awesome Free](https://fontawesome.com) (icons CC BY 4.0, font SIL OFL 1.1), bundled in `web/fa/`.
