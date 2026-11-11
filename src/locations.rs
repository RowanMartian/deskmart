//! Where to look, and the special cases (port of Locations.swift and Conditions.swift).

use crate::util::*;

/// Standard Library subfolders. A depth-2 match under one of these adds the item itself;
/// under anything else (a vendor folder like /Library/Objective-See) it adds the vendor folder.
pub const STANDARD_LIBRARY_SUBDIRS: &[&str] = &[
    "Application Scripts", "Application Support", "Caches", "Containers", "Group Containers", "HTTPStorages", "Internet Plug-Ins",
    "LaunchAgents", "LaunchDaemons", "Logs", "Preferences", "PreferencePanes", "PrivilegedHelperTools", "Saved Application State",
    "Services", "WebKit", "Extensions", "Frameworks",
];

pub fn darwin_dirs() -> (String, String) {
    if !cfg!(target_os = "macos") {
        return (String::new(), String::new());
    }
    let get = |k: &str| cmd_quick("/usr/bin/getconf", &[k]).stdout.trim().trim_end_matches('/').to_string();
    (get("DARWIN_USER_CACHE_DIR"), get("DARWIN_USER_TEMP_DIR"))
}

fn app_support_subfolders() -> Vec<String> {
    let base = format!("{}/Library/Application Support", home());
    let skip = [
        "MobileSync", ".DS_Store", "Xcode", "SyncServices", "networkserviceproxy", "DiskImages", "CallHistoryTransactions", "App Store", "CloudDocs", "icdd",
        "iCloud", "Instruments", "AddressBook", "FaceTime", "AskPermission", "CallHistoryDB",
    ];
    list_dir(&base)
        .into_iter()
        .filter(|n| !skip.contains(&n.as_str()) && !n.starts_with("com.apple.") && is_dir(&format!("{base}/{n}")))
        .map(|n| format!("{base}/{n}"))
        .collect()
}

pub fn app_locations() -> Vec<String> {
    let mut v = fixed_roots();
    v.extend(app_support_subfolders());
    v.dedup();
    v
}

/// The fixed search roots (never offered for removal themselves). The per-app Application Support
/// subfolders added by `app_locations` are searched too, but are valid leftovers.
pub fn fixed_roots() -> Vec<String> {
    let (cache, temp) = darwin_dirs();
    let mut v: Vec<String> = [
        "~", "~/.config", "~/Documents", "~/Desktop", "~/Applications", "~/Library", "~/Library/Application Scripts", "~/Library/Application Support",
        "~/Library/Application Support/CrashReporter", "~/Library/Application Support/Steam/steamapps", "~/Library/Application Support/Steam/steamapps/common",
        "~/Library/Application Support/com.apple.sharedfilelist/com.apple.LSSharedFileList.ApplicationRecentDocuments", "~/Library/Containers", "~/Library/Caches",
        "~/Library/Caches/com.apple.helpd/Generated", "~/Library/Caches/com.crashlytics", "~/Library/Caches/com.google.SoftwareUpdate",
        "~/Library/Caches/com.google.Keystone", "~/Library/Caches/org.sparkle-project.Sparkle", "~/Library/Caches/com.segment.analytics",
        "~/Library/Caches/SentryCrash", "~/Library/Caches/Rollbar", "~/Library/Caches/Amplitude", "~/Library/Caches/Realm", "~/Library/Caches/Parse",
        "~/Library/Group Containers", "~/Library/HTTPStorages", "~/Library/Internet Plug-Ins", "~/Library/LaunchAgents", "~/Library/Logs",
        "~/Library/Logs/DiagnosticReports", "~/Library/Preferences", "~/Library/PreferencePanes", "~/Library/Preferences/ByHost",
        "~/Library/Saved Application State", "~/Library/Services", "~/Library/WebKit", "/Applications", "/Users/Shared", "/Users/Library",
        "/Users/Shared/Library/Application Support", "/Library", "/Library/Application Support", "/Library/Application Support/CrashReporter", "/Library/Caches",
        "/Library/Extensions", "/Library/Internet Plug-Ins", "/Library/LaunchAgents", "/Library/LaunchDaemons", "/Library/Logs", "/Library/Logs/DiagnosticReports",
        "/Library/Preferences", "/Library/PrivilegedHelperTools", "/private/var/db/receipts", "/private/tmp", "/usr/local/bin", "/usr/local/etc", "/usr/local/opt",
        "/usr/local/sbin", "/usr/local/share", "/usr/local/var",
    ]
    .iter()
    .map(|p| expand(p))
    .collect();
    for d in [cache, temp] {
        if !d.is_empty() {
            v.push(d);
        }
    }
    v
}

pub fn reverse_locations() -> Vec<String> {
    [
        "~/Library/Application Scripts", "~/Library/Application Support", "~/Library/Application Support/Caches",
        "~/Library/Application Support/com.apple.sharedfilelist/com.apple.LSSharedFileList.ApplicationRecentDocuments", "~/Library/Containers", "~/Library/Caches",
        "~/Library/HTTPStorages", "~/Library/Internet Plug-Ins", "~/Library/LaunchAgents", "~/Library/Logs", "~/Library/Preferences", "~/Library/PreferencePanes",
        "~/Library/Preferences/ByHost", "~/Library/Saved Application State", "~/Library/WebKit", "/Users/Shared/Library/Application Support", "/Library/Application Support",
        "/Library/Application Support/CrashReporter", "/Library/Internet Plug-Ins", "/Library/LaunchAgents", "/Library/LaunchDaemons", "/Library/PrivilegedHelperTools",
    ]
    .iter()
    .map(|p| expand(p))
    .collect()
}

pub fn plugin_locations() -> Vec<(&'static str, Vec<String>)> {
    let e = |v: &[&str]| v.iter().map(|p| expand(p)).collect::<Vec<_>>();
    vec![
        ("Audio", e(&[
            "~/Library/Audio/Plug-Ins/Components", "~/Library/Audio/Plug-Ins/HAL", "~/Library/Audio/Plug-Ins/MAS", "~/Library/Audio/Plug-Ins/VST",
            "~/Library/Audio/Plug-Ins/VST3", "~/Library/Audio/Plug-Ins/CLAP", "/Library/Audio/Plug-Ins/HAL", "/Library/Audio/Plug-Ins/VST",
            "/Library/Audio/Plug-Ins/VST3", "/Library/Audio/Plug-Ins/CLAP", "/Library/Audio/Plug-Ins/Components",
            "/Library/Application Support/Avid/Audio/Plug-Ins", "/Library/Application Support/Digidesign/Plug-Ins",
        ])),
        ("PreferencePanes", e(&["/Library/PreferencePanes", "~/Library/PreferencePanes"])),
        ("QuickLook", e(&["/Library/QuickLook", "~/Library/QuickLook"])),
        ("Screen Savers", e(&["/Library/Screen Savers", "~/Library/Screen Savers"])),
        ("Internet Plug-Ins", e(&["/Library/Internet Plug-Ins", "~/Library/Internet Plug-Ins"])),
        ("Core Image", e(&["/Library/CoreImage", "~/Library/CoreImage"])),
        ("ColorPickers", e(&["/Library/ColorPickers", "~/Library/ColorPickers"])),
        ("Fonts", e(&["~/Library/Fonts"])),
        ("Dictionaries", e(&["/Library/Dictionaries", "~/Library/Dictionaries"])),
        ("Automator", e(&["/Library/Automator", "~/Library/Automator"])),
        ("Safari Extensions", e(&["/Library/Safari/Extensions", "~/Library/Safari/Extensions"])),
        ("Motion Templates", e(&["~/Movies/Motion Templates", "/Library/Application Support/Final Cut Pro System Support/Plug-ins"])),
        ("Spotlight", e(&["/Library/Spotlight", "~/Library/Spotlight"])),
        ("Services", e(&["/Library/Services", "~/Library/Services"])),
        ("Address Book", e(&["~/Library/Address Book Plug-Ins"])),
        ("Contextual Menu", e(&["/Library/Contextual Menu Items", "~/Library/Contextual Menu Items"])),
        ("Input Methods", e(&["/Library/Input Methods", "~/Library/Input Methods"])),
        ("Widgets", e(&["/Library/Widgets", "~/Library/Widgets"])),
    ]
}

// ------------------------------------------------------------------ per-app conditions

pub struct Condition {
    pub bundle_id: String,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub include_force: Vec<String>,
}

fn cond(bundle: &str, include: &[&str], exclude: &[&str], force: &[&str]) -> Condition {
    Condition {
        bundle_id: pear_format(bundle),
        include: include.iter().map(|x| pear_format(x)).collect(),
        exclude: exclude.iter().map(|x| pear_format(x)).collect(),
        include_force: force.iter().map(|p| expand(p).trim_end_matches('/').to_string()).filter(|p| exists(p)).collect(),
    }
}

pub fn conditions() -> Vec<Condition> {
    vec![
        cond("com.apple.dt.xcode", &["com.apple.dt", "xcode", "simulator"],
            &["com.robotsandpencils.xcodesapp", "com.xcodesorg.xcodesapp", "com.oneminutegames.xcodecleaner", "io.hyperapp.xcodecleaner", "available-xcodes", "xcodes", "cleaner for xcode"],
            &["~/Library/Containers/com.apple.iphonesimulator.ShareExtension"]),
        cond("com.robotsandpencils.xcodesapp", &[], &["com.apple.dt.xcode", "com.oneminutegames.xcodecleaner", "io.hyperapp.xcodecleaner"], &[]),
        cond("com.xcodesorg.xcodesapp", &[], &["com.apple.dt.xcode", "com.oneminutegames.xcodecleaner", "io.hyperapp.xcodecleaner"], &[]),
        cond("io.hyperapp.xcodecleaner", &[], &["com.robotsandpencils.xcodesapp", "com.oneminutegames.xcodecleaner", "com.apple.dt.xcode", "xcodes.json"], &[]),
        cond("us.zoom.xos", &["zoom"], &[], &[]),
        cond("com.brave.browser", &["brave"], &[], &[]),
        cond("com.okta.mobile", &["okta"], &[], &[]),
        cond("com.google.chrome", &["google", "chrome"], &["iterm", "chromefeaturestate", "monochrome"], &[]),
        cond("com.microsoft.edgemac", &[], &["vscode", "rdc", "appcenter", "office", "oneauth"], &[]),
        cond("com.microsoft.teams2", &[], &["office"], &[]),
        cond("org.mozilla.firefox", &["firefox"], &["thunderbird"], &[]),
        cond("org.mozilla.thunderbird", &[], &["firefox"], &[]),
        cond("org.mozilla.firefox.nightly", &["mozilla", "firefox"], &["thunderbird"], &[]),
        cond("com.logi.optionsplus", &["logi", "logipluginservice"], &["login", "logic"], &[]),
        cond("com.microsoft.VSCode", &["vscode"], &["vscodeinsiders", "insiders"], &["~/Library/Application Support/Code/"]),
        cond("com.microsoft.VSCodeInsiders", &["vscodeinsiders", "insiders"], &[], &["~/Library/Application Support/Code - Insiders/"]),
        cond("com.facebook.archon.developerid", &["archon.loginhelper"], &[], &[]),
        cond("eu.exelban.stats", &[], &["video"], &[]),
        cond("me.mhaeuser.BatteryToolkit", &["memhaeuser"], &[], &[]),
        cond("jetbrains", &["jcef"], &[], &["~/Library/Application Support/JetBrains/", "~/Library/Caches/JetBrains/", "~/Library/Logs/JetBrains/"]),
        cond("company.thebrowser.Browser", &["firestore"], &[], &["~/Library/Application Support/Arc/", "~/Library/Caches/Arc/"]),
        cond("com.1password.1password", &["waveboxapp", "sidekick"], &[], &[]),
        cond("com.now.gg.BlueStacks", &["bst_boost_interprocess"], &[], &[]),
        cond("com.electron.sdm", &["strongdm"], &[], &[]),
        cond("com.github.githubclient", &["comgithubelectron"], &[], &[]),
        cond("com.native-instruments.nativeaccess", &["comnative", "nativeinstruments"], &[], &[]),
    ]
}

// ------------------------------------------------------------------ skip lists

pub const SKIP_PREFIX: &[&str] = &["mobiledocuments", "reminders", "dsstore", "comapplepasswordmanager"];
pub const ALLOW_PREFIX: &[&str] =
    &["comappleconfigurator", "comappledt", "comappleiwork", "comapplesfsymbols", "comappletestflight", "comapplesharedfilelist", "comapplelssharedfilelist"];
pub fn skip_paths() -> Vec<String> {
    vec![
        expand("~/.Trash"),
        "/Library/SystemExtensions".into(),
        "/System/Volumes/Preboot/Cryptexes/App/System/Library/CoreServices/PasswordManagerBrowserExtensionHelper.app/Contents/MacOS/PasswordManagerBrowserExtensionHelper".into(),
        expand("~/Library/Application Support/Chromium/NativeMessagingHosts/com.apple.passwordmanager.json"),
        expand("~/Library/Application Support/Google/Chrome/NativeMessagingHosts/com.apple.passwordmanager.json"),
        // Deskmart's own settings
        data_dir().to_string_lossy().to_string(),
    ]
}

/// Library subfolders never searched two levels deep (macOS's own data).
pub const SKIP_DEEP_SEARCH: &[&str] = &[
    "Apple", "Audio", "Bluetooth", "ColorSync", "Components", "CoreAnalytics", "CoreMediaIO", "DirectoryServices", "Filesystems", "GPUBundles", "Graphics",
    "KernelCollections", "OSAnalytics", "OpenDirectory", "Sandbox", "Security", "SystemExtensions", "SystemMigration", "SystemProfiler", "StagedDriverExtensions",
    "StagedExtensions", "StartupItems", "Accessibility", "Accounts", "AppleMediaServices", "Assistant", "Assistants", "Autosave Information", "Biome", "Calendars",
    "CallServices", "CloudStorage", "Contacts", "Cookies", "DataAccess", "DataDeliveryServices", "DoNotDisturb", "DuetExpertCenter", "Finance", "FinanceBackup",
    "FrontBoard", "GameKit", "GroupContainersAlias", "HomeKit", "IdentityServices", "IntelligencePlatform", "Intents", "KeyboardServices", "LanguageModeling",
    "LockdownMode", "Mail", "MediaAnalysis", "Messages", "Metadata", "Mobile Documents", "MobileDevice", "News", "Passes", "PersonalizationPortrait", "Photos",
    "PrivateCloudCompute", "Reminders", "ResponseKit", "Safari", "SafariSafeBrowsing", "SafariSandboxBroker", "ScreenRecordings", "StatusKit", "Suggestions",
    "SyncedPreferences", "Translation", "UnifiedAssetFramework", "Weather", "homeenergyd", "studentd", "Developer", "Perl", "Ruby", "Java", "Python", "Catacomb",
    "InstallerSandboxes", "Trial", "Updates", "Staging", "ContainerManager", "Daemon Containers", "ColorPickers", "Colors", "Compositions", "Contextual Menu Items",
    "Documentation", "DriverExtensions", "Favorites", "FontCollections", "Fonts", "Image Capture", "Input Methods", "Jupyter", "Keyboard", "Keyboard Layouts",
    "Keychains", "Managed Preferences", "PDF Services", "Printers", "QuickLook", "Receipts", "Screen Savers", "ScriptingAdditions", "Scripts", "Sharing", "Shortcuts",
    "Sounds", "Speech", "Spelling", "Spotlight", "User Pictures", "User Template", "Video", "WebServer", "Workflows", "com.apple.AppleMediaServices",
    "com.apple.WatchListKit", "com.apple.aiml.instrumentation", "com.apple.appleaccountd", "com.apple.bluetooth.services.cloud", "com.apple.bluetoothuser",
    "com.apple.familycircled", "com.apple.iTunesCloud", "com.apple.internal.ck",
    // macOS 27 Golden Gate system data
    "Intelligence", "SiriKit", "GenerativeModels", "Visual Intelligence",
];

/// Names skipped in the orphaned-file search.
pub const SKIP_REVERSE: &[&str] = &[
    "apple", "temporary", "btserver", "proapps", "scripteditor", "ilife", "livefsd", "siritoday", "addressbook", "animoji", "appstore", "askpermission", "callhistory",
    "clouddocs", "diskimages", "dock", "facetime", "fileprovider", "instruments", "knowledge", "mobilesync", "syncservices", "homeenergyd", "icloud", "icdd",
    "networkserviceproxy", "familycircle", "geoservices", "installation", "passkit", "sharedimagecache", "desktop", "mbuseragent", "swiftpm", "baseband",
    "coresimulator", "photoslegacyupgrade", "photosupgrade", "siritts", "ipod", "globalpreferences", "apmanalytics", "apmexperiment", "avatarcache", "byhost",
    "contextstoreagent", "mobilemeaccounts", "mobiledocuments", "mobile", "intentbuilderc", "loginwindow", "momc", "replayd", "sharedfilelistd", "clang",
    "audiocomponent", "csexattrcryptoservice", "livetranscriptionagent", "sandboxhelper", "statuskitagent", "betaenrollmentd", "contentlinkingd",
    "diagnosticextensionsd", "gamed", "heard", "homed", "itunescloudd", "lldb", "mds", "mediaanalysisd", "metrickitd", "mobiletimerd", "proactived", "ptpcamerad",
    "studentd", "talagent", "watchlistd", "apptranslocation", "xcrun", "ds_store", "caches", "crashreporter", "trash", "pearcleaner", "deskmart",
    "amsdatamigratortool", "arfilecache", "assistant", "chromium", "cloudkit", "webkit", "databases", "diagnostic", "cache", "gamekit", "homebrew", "logi",
    "microsoft", "mozilla", "sync", "google", "sentinel", "hexnode", "sentry", "tvappservices", "reminders", "pbs", "notarytool", "differentialprivacy",
    "storeassetd", "webpush", "storedownloadd", "fsck", "crash", "python", "discrecording", "photossearch", "pylint", "jamf", "scopedbookmarkagent", "anonymous",
    "identifier", "isolated", "nobackup", "privacypreservingmeasurement", "symbols", "stickersd", "privatecloudcomputed", "tipsd", "controlcenter", "contactsd",
    "staticcheck", "index", "segment", "sparkle", "summaryevents", "launchdarkly", "identityservicesd", "embeddedbinaryvalidationutility", "comalienator88",
    "aaprofilepicture", "minilauncher", "jna", "automator", "locationaccessstored", "spotlight", "cef",
    // macOS 27 Golden Gate system services
    "intelligence", "generativemodels", "siri", "visualintelligence",
];
