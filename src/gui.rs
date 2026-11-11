//! The app window: a native window with the system WebView (WKWebView on macOS) showing the local UI.
//! Same structure as Dropfin's window.

use crate::api::{self, App};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

enum UserEvent {
    #[cfg(target_os = "macos")]
    Menu(muda::MenuEvent),
    Ipc(String),
    Run(Box<dyn FnOnce() + Send>),
    Quit,
}

/// Window dragging + no browser context menu. Runs before the page's own scripts.
const INIT_JS: &str = r#"
window.__DESKMART_APP__ = true;
(() => {
  const DRAG = '.toolbar, .sb-head, .welcome';
  const NO = 'button, input, label, a, select, textarea, .seg, .scrim, .menu, .chip, .tb-title';
  document.addEventListener('mousedown', (e) => {
    if (e.button !== 0 || !e.target.closest) return;
    if (!e.target.closest(DRAG) || e.target.closest(NO)) return;
    e.preventDefault();
    window.ipc.postMessage(e.detail === 2 ? 'zoom' : 'drag');
  });
  document.addEventListener('contextmenu', (e) => {
    if (!(e.target.closest && e.target.closest('input, textarea'))) e.preventDefault();
  });
})();
"#;

#[derive(serde::Serialize, serde::Deserialize)]
struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn fatal(msg: &str) -> ! {
    eprintln!("Deskmart couldn't start: {msg}");
    #[cfg(target_os = "macos")]
    rfd::MessageDialog::new().set_level(rfd::MessageLevel::Error).set_title("Deskmart couldn't start").set_description(msg).show();
    std::process::exit(1);
}

pub fn run(app: Arc<App>, port: u16, initial: Vec<String>) {
    let (tx, rx) = std::sync::mpsc::channel::<Result<u16, String>>();
    {
        let app = app.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            let tx2 = tx.clone();
            if let Err(e) = rt.block_on(api::serve(app, port, move |p| {
                let _ = tx2.send(Ok(p));
            })) {
                let _ = tx.send(Err(e.to_string()));
            }
        });
    }
    let port = match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => fatal(&e),
        Err(_) => fatal("The helper didn't start in time."),
    };

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    {
        let p = proxy.clone();
        *app.quit.lock().unwrap() = Some(Box::new(move || {
            let _ = p.send_event(UserEvent::Quit);
        }));
    }
    {
        let p = Mutex::new(proxy.clone());
        crate::icons::set_main_runner(Box::new(move |f| {
            let _ = p.lock().unwrap().send_event(UserEvent::Run(f));
        }));
    }

    // ---- window
    let bounds_file = app.data_dir.join("window.json");
    let saved: Option<Bounds> = std::fs::read_to_string(&bounds_file).ok().and_then(|t| serde_json::from_str(&t).ok());
    let (w, h) = saved.as_ref().map(|b| (b.width, b.height)).unwrap_or((1180.0, 760.0));
    let mut wb = WindowBuilder::new()
        .with_title("Deskmart")
        .with_inner_size(LogicalSize::new(w.max(860.0), h.max(520.0)))
        .with_min_inner_size(LogicalSize::new(860.0, 520.0));
    if let Some(b) = &saved {
        wb = wb.with_position(LogicalPosition::new(b.x, b.y));
    }
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::WindowBuilderExtMacOS;
        wb = wb
            .with_titlebar_transparent(true)
            .with_title_hidden(true)
            .with_fullsize_content_view(true)
            .with_traffic_light_inset(LogicalPosition::new(18.0, 19.0));
    }
    let window = wb.build(&event_loop).unwrap_or_else(|e| fatal(&e.to_string()));

    #[cfg(target_os = "macos")]
    let _menu = {
        let p = proxy.clone();
        muda::MenuEvent::set_event_handler(Some(move |e: muda::MenuEvent| {
            let _ = p.send_event(UserEvent::Menu(e));
        }));
        build_menu()
    };

    // ---- webview
    let origin = format!("http://localhost:{port}/");
    let dark = window.theme() == tao::window::Theme::Dark;
    let ipc_proxy = proxy.clone();
    let nav_origin = origin.clone();
    let drop_proxy = Mutex::new(proxy.clone());
    let builder = WebViewBuilder::new()
        .with_url(origin.clone())
        .with_initialization_script(INIT_JS)
        .with_ipc_handler(move |req| {
            let _ = ipc_proxy.send_event(UserEvent::Ipc(req.body().clone()));
        })
        .with_navigation_handler(move |url| url.starts_with(&nav_origin))
        .with_drag_drop_handler(move |e| {
            if let wry::DragDropEvent::Drop { paths, .. } = e {
                let list: Vec<String> = paths.iter().map(|p| p.to_string_lossy().to_string()).collect();
                let _ = drop_proxy.lock().unwrap().send_event(UserEvent::Ipc(format!("drop:{}", serde_json::to_string(&list).unwrap_or_default())));
            }
            true
        })
        .with_background_color(if dark { (35, 35, 37, 255) } else { (245, 245, 247, 255) })
        .with_devtools(cfg!(debug_assertions));
    #[cfg(not(target_os = "linux"))]
    let webview = builder.build(&window).unwrap_or_else(|e| fatal(&e.to_string()));
    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        builder.build_gtk(window.default_vbox().expect("gtk vbox")).unwrap_or_else(|e| fatal(&e.to_string()))
    };

    let save_bounds = {
        let file = bounds_file.clone();
        move |window: &tao::window::Window| {
            if window.is_maximized() || window.is_minimized() {
                return;
            }
            let scale = window.scale_factor();
            if let (Ok(pos), size) = (window.outer_position(), window.inner_size()) {
                let pos = pos.to_logical::<f64>(scale);
                let size = size.to_logical::<f64>(scale);
                let b = Bounds { x: pos.x, y: pos.y, width: size.width, height: size.height };
                let _ = std::fs::write(&file, serde_json::to_string(&b).unwrap_or_default());
            }
        }
    };

    // Apps/links handed to us before the page is ready wait here.
    let mut page_ready = false;
    let mut pending: Vec<String> = initial;
    let app_ev = app.clone();
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let _ = &webview;
        let quit = |control_flow: &mut ControlFlow, ask: bool| {
            if ask && app_ev.busy() && !confirm_quit(&window) {
                return;
            }
            save_bounds(&window);
            *control_flow = ControlFlow::Exit;
        };
        let deliver = |items: &[String]| {
            let js = format!("window.deskmartOpen && window.deskmartOpen({})", serde_json::to_string(items).unwrap_or_default());
            let _ = webview.evaluate_script(&js);
        };
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => quit(control_flow, true),
            Event::UserEvent(UserEvent::Quit) => quit(control_flow, false),
            Event::UserEvent(UserEvent::Run(f)) => f(),
            #[cfg(target_os = "macos")]
            Event::Opened { urls } => {
                let items: Vec<String> = urls.iter().map(|u| if u.scheme() == "file" { u.to_file_path().map(|p| p.to_string_lossy().to_string()).unwrap_or_default() } else { u.to_string() }).collect();
                window.set_visible(true);
                window.set_focus();
                if page_ready {
                    deliver(&items);
                } else {
                    pending.extend(items);
                }
            }
            Event::UserEvent(UserEvent::Ipc(msg)) => match msg.as_str() {
                "drag" => {
                    let _ = window.drag_window();
                }
                "zoom" => window.set_maximized(!window.is_maximized()),
                "ready" => {
                    page_ready = true;
                    if !pending.is_empty() {
                        deliver(&pending);
                        pending.clear();
                    }
                }
                m if m.starts_with("drop:") => {
                    let list: Vec<String> = serde_json::from_str(&m[5..]).unwrap_or_default();
                    deliver(&list);
                }
                _ => {}
            },
            #[cfg(target_os = "macos")]
            Event::UserEvent(UserEvent::Menu(e)) => match e.id.0.as_str() {
                "quit" => quit(control_flow, true),
                "dataFolder" => {
                    let _ = std::process::Command::new("/usr/bin/open").arg(&app_ev.data_dir).spawn();
                }
                id => {
                    let js = format!("window.deskmartCommand && window.deskmartCommand({})", serde_json::to_string(id).unwrap_or_default());
                    let _ = webview.evaluate_script(&js);
                }
            },
            _ => {}
        }
    });
}

#[cfg(target_os = "macos")]
fn confirm_quit(window: &tao::window::Window) -> bool {
    let r = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("Deskmart is still working")
        .set_description("A scan or Homebrew task is running. If you quit now, it stops.")
        .set_buttons(rfd::MessageButtons::OkCancelCustom("Stop and Quit".into(), "Keep Working".into()))
        .set_parent(window)
        .show();
    matches!(r, rfd::MessageDialogResult::Custom(ref s) if s == "Stop and Quit") || matches!(r, rfd::MessageDialogResult::Ok)
}
#[cfg(not(target_os = "macos"))]
fn confirm_quit(_window: &tao::window::Window) -> bool {
    true
}

#[cfg(target_os = "macos")]
fn build_menu() -> muda::Menu {
    use muda::{accelerator::Accelerator, AboutMetadata, Menu, MenuItem, PredefinedMenuItem as P, Submenu};
    let item = |id: &str, text: &str, accel: Option<&str>| MenuItem::with_id(id, text, true, accel.and_then(|a| a.parse::<Accelerator>().ok()));
    let sep = || P::separator();

    let version = match api::BUILD_COMMIT {
        Some(c) if c.len() >= 7 => format!("{} ({})", api::VERSION, &c[..7]),
        _ => api::VERSION.to_string(),
    };
    let about = AboutMetadata {
        name: Some("Deskmart".into()),
        version: Some(version),
        comments: Some("Uninstall apps and everything they leave behind.\nA Rust rebuild of Pearcleaner by alienator88.".into()),
        ..Default::default()
    };
    let app_menu = Submenu::with_items("Deskmart", true, &[
        &P::about(None, Some(about)),
        &item("checkUpdates", "Check for Updates…", None), &sep(),
        &item("settings", "Settings…", Some("CmdOrCtrl+Comma")),
        &item("permissions", "Permissions…", None), &sep(),
        &P::services(None), &sep(), &P::hide(None), &P::hide_others(None), &P::show_all(None), &sep(),
        &item("quit", "Quit Deskmart", Some("CmdOrCtrl+Q")),
    ]).unwrap();
    let file = Submenu::with_items("File", true, &[
        &item("uninstall", "Uninstall…", Some("CmdOrCtrl+Backspace")),
        &item("reveal", "Show in Finder", Some("CmdOrCtrl+Shift+R")),
        &item("export", "Export File List", None), &sep(),
        &item("history", "Restore Deleted Files…", Some("Alt+CmdOrCtrl+Z")),
        &item("emptyTrash", "Empty Trash…", None), &sep(),
        &P::close_window(None),
    ]).unwrap();
    let edit = Submenu::with_items("Edit", true, &[
        &P::undo(None), &P::redo(None), &sep(), &P::cut(None), &P::copy(None), &P::paste(None), &P::select_all(None), &sep(),
        &item("find", "Find", Some("CmdOrCtrl+F")),
    ]).unwrap();
    let view = Submenu::with_items("View", true, &[
        &item("page:apps", "Apps", Some("CmdOrCtrl+1")),
        &item("page:orphans", "Orphans", Some("CmdOrCtrl+2")),
        &item("page:development", "Developer", Some("CmdOrCtrl+3")),
        &item("page:fileSearch", "File Search", Some("CmdOrCtrl+4")),
        &item("page:homebrew", "Homebrew", Some("CmdOrCtrl+5")),
        &item("page:lipo", "Lipo", Some("CmdOrCtrl+6")),
        &item("page:packages", "Packages", Some("CmdOrCtrl+7")),
        &item("page:plugins", "Plugins", Some("CmdOrCtrl+8")),
        &item("page:services", "Services", Some("CmdOrCtrl+9")),
        &item("page:updater", "Updater", Some("CmdOrCtrl+0")), &sep(),
        &item("viewGrid", "as Icons", None),
        &item("viewList", "as List", None),
        &item("console", "Show Console", Some("CmdOrCtrl+Shift+C")), &sep(),
        &item("refresh", "Refresh", Some("CmdOrCtrl+R")), &sep(),
        &P::fullscreen(None),
    ]).unwrap();
    let window = Submenu::with_items("Window", true, &[&P::minimize(None), &P::maximize(None), &sep(), &P::fullscreen(None)]).unwrap();
    let help = Submenu::with_items("Help", true, &[&item("dataFolder", "Show Settings Folder", None)]).unwrap();

    let menu = Menu::with_items(&[&app_menu, &file, &edit, &view, &window, &help]).unwrap();
    menu.init_for_nsapp();
    window.set_as_windows_menu_for_nsapp();
    help.set_as_help_menu_for_nsapp();
    menu
}
