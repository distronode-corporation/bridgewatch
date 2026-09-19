//! bridgewatch's tray application.
//!
//! A thin shell: it owns windows, a tray icon and the OS integrations, and
//! nothing else. Every verdict, every poll decision and every validation comes
//! from `bridgewatch-core`, which is what lets the interesting half be tested
//! against recorded fixtures with no display server anywhere near it.

#![forbid(unsafe_code)]
// A tray app has no console to show. Windows is not a 0.1 target, but the
// attribute costs nothing and stops a stray terminal the day it is.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod guard;
mod icons;
mod links;
mod logging;
mod oauth;
mod poller;
mod reorder;
mod state;
mod tray;
mod windows;
mod wizard;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use notify::Watcher;
use tauri::{Manager, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_notification::NotificationExt;

use state::AppState;

/// The file watcher, parked for the process lifetime.
///
/// ⛔ Dropping a `notify` watcher stops the watch, and it does so silently: hot
/// reload would simply never fire again, with nothing in any log to say why.
static WATCHER: OnceLock<Mutex<Box<dyn Watcher + Send>>> = OnceLock::new();

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return;
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("bridgewatch {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let explicit = match parse_config_arg(&args) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("bridgewatch: {message}");
            std::process::exit(2);
        }
    };

    // stderr only: a tray app has nowhere else to write. `RUST_LOG` wins when
    // set; otherwise `[log].level` applies once the file has loaded.
    logging::init();

    // Nothing is written here. A first launch (no file at the default path)
    // opens the setup wizard, which writes the file when it is finished.
    let resolved = config::resolve(explicit.as_deref());
    let first_run = resolved.first_run;
    let config_ok = resolved.validation.ok;
    if let Some(loaded) = &resolved.loaded {
        logging::apply_config_level(&loaded.config.log.level);
    }
    // After the level is applied, so `[log].level = "info"` shows it. One
    // line, so a bug report's log says which file was in force.
    tracing::info!("{}", logging::startup_line(&resolved));
    let state = Arc::new(AppState::new(resolved));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_positioner::init())
        // `LaunchAgent` writes a plist under ~/Library/LaunchAgents rather than
        // using the App Store-era SMLoginItem API, which needs a helper bundle
        // this app does not have. No arguments: a login start is an ordinary
        // start.
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(Vec::<&str>::new()),
        ))
        .manage(state.clone())
        .manage(wizard::WizardSession::default())
        .manage(oauth::SignIns::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::get_status,
            commands::refresh_now,
            commands::popover_opened,
            commands::read_config_text,
            commands::get_config_json,
            commands::move_watch,
            commands::read_theme_css,
            commands::validate_config_text,
            commands::save_config_text,
            commands::apply_config_edits,
            commands::reload_config,
            commands::open_config_file,
            commands::pipelines_url,
            commands::open_link,
            commands::open_settings,
            commands::take_settings_tab,
            commands::hide_popover,
            commands::resize_popover,
            commands::quit,
            commands::set_own_token,
            commands::clear_own_token,
            commands::get_launch_at_login,
            commands::set_launch_at_login,
            wizard::wizard_detect_cli_token,
            wizard::wizard_test_connection,
            wizard::wizard_list_projects,
            wizard::wizard_resolve_project,
            wizard::wizard_suggest_deploy_markers,
            wizard::wizard_preview_config,
            wizard::wizard_save,
            wizard::wizard_skip,
            wizard::open_wizard,
            oauth::oauth_availability,
            oauth::oauth_start,
            oauth::oauth_wait,
            oauth::oauth_cancel,
            oauth::oauth_open_verification,
            oauth::oauth_open_install,
            oauth::oauth_status,
            oauth::oauth_sign_out,
        ])
        .on_window_event(on_window_event)
        .setup(move |app| {
            // ⛔ No Dock icon and no application menu: bridgewatch lives in the
            // menu bar. Without this the popover cannot be shown without the
            // app also stealing focus as a foreground application.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            tray::build(app.handle())?;
            windows::apply_vibrancy(app.handle());

            // ⚠ A no-op on desktop today: `Notification::permission_state()`
            // returns `Granted` unconditionally in the plugin's desktop
            // implementation, so this never asks. It is here because the mobile
            // implementation does ask, and because an OS that refuses should
            // still leave a working tray behind — never fatal.
            let notifier = app.notification();
            if !matches!(
                notifier.permission_state(),
                Ok(tauri_plugin_notification::PermissionState::Granted)
            ) {
                let _ = notifier.request_permission();
            }

            let handle = app.handle().clone();
            let watch_state = state.clone();
            if let Some(watcher) = config::watch(&watch_state.config_path.clone(), move || {
                commands::reload_from_disk(&handle, false);
            }) {
                let _ = WATCHER.set(Mutex::new(watcher));
            } else {
                tracing::warn!("could not watch the configuration file; hot reload is off");
            }

            let handle = app.handle().clone();
            let task_state = state.clone();
            tauri::async_runtime::spawn(poller::run(handle, task_state));

            // A first run has nothing to show in the popover and no file, so
            // it opens the setup wizard. A file that exists but does not parse
            // opens the text editor tab, the only place it can be fixed.
            if first_run {
                wizard::show_wizard(app.handle());
            } else if !config_ok {
                windows::show_settings(app.handle(), Some("text"));
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("bridgewatch failed to start")
        .run(|app, event| match event {
            // Hiding a window rather than closing it means the last window
            // closing never ends the process; this makes that explicit so a
            // future change to the close handling cannot quietly exit the tray.
            tauri::RunEvent::ExitRequested { api, code, .. } if code.is_none() => {
                api.prevent_exit();
            }
            // Linux: the tray's image files live in a per-run directory that
            // only this removes (a killed run's is swept at the next start).
            tauri::RunEvent::Exit => tray::remove_icon_dir(app),
            _ => {}
        });
}

/// Hide-on-blur, and close-means-hide for both windows.
fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    match event {
        // ⛔ Never let a close destroy a window. Both are declared in
        // tauri.conf.json and created at startup precisely so that opening the
        // popover is instant; destroying one would put a webview boot between
        // the next click and anything appearing.
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = window.hide();
            if window.label() == windows::POPOVER {
                windows::note_popover_hidden(&window.app_handle().clone());
            }
        }
        WindowEvent::Focused(false) if window.label() == windows::POPOVER => {
            let app = window.app_handle().clone();
            let hide = app
                .state::<Arc<AppState>>()
                .config()
                .map(|c| c.ui.popover.hide_on_blur)
                .unwrap_or(true);
            if hide {
                let _ = window.hide();
                windows::note_popover_hidden(&app);
            }
        }
        _ => {}
    }
}

/// `--config <path>`, `--config=<path>` or `-c <path>`.
///
/// Hand-rolled rather than clap: this binary takes exactly one option, and a
/// GUI shell pulling in an argument parser for it would be the tail wagging
/// the dog. The CLI, which has real subcommands, uses clap.
fn parse_config_arg(args: &[String]) -> Result<Option<PathBuf>, String> {
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if let Some(rest) = arg.strip_prefix("--config=") {
            return Ok(Some(PathBuf::from(rest)));
        }
        if arg == "--config" || arg == "-c" {
            return match iter.next() {
                Some(path) => Ok(Some(PathBuf::from(path))),
                None => Err(format!("{arg} needs a path")),
            };
        }
        if arg.starts_with('-') {
            return Err(format!("unknown option {arg:?}; try --help"));
        }
    }
    Ok(None)
}

fn print_usage() {
    println!(
        "bridgewatch {} — bridge-aware GitLab CI tray monitor

USAGE:
    bridgewatch [--config <PATH>]

OPTIONS:
    -c, --config <PATH>    Configuration file. Falls back to $BRIDGEWATCH_CONFIG,
                           then <config dir>/bridgewatch/config.toml. When that
                           does not exist, the setup wizard opens to write it.
    -h, --help             Print this.
    -V, --version          Print the version.

ENVIRONMENT:
    BRIDGEWATCH_CONFIG     Same as --config, lower precedence.
    RUST_LOG               Tracing filter. When set and non-empty it wins over
                           [log].level in the file; with neither, `warn`.

The command-line companion with subcommands is `bridgewatch` from
bridgewatch-cli; this binary is the tray application.",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use super::parse_config_arg;
    use std::path::PathBuf;

    #[test]
    fn config_arg_is_accepted_in_all_three_spellings() {
        for args in [
            vec!["--config".to_string(), "/tmp/a.toml".to_string()],
            vec!["--config=/tmp/a.toml".to_string()],
            vec!["-c".to_string(), "/tmp/a.toml".to_string()],
        ] {
            assert_eq!(
                parse_config_arg(&args).unwrap(),
                Some(PathBuf::from("/tmp/a.toml")),
                "{args:?}"
            );
        }
    }

    #[test]
    fn no_argument_means_the_resolver_decides() {
        assert_eq!(parse_config_arg(&[]).unwrap(), None);
    }

    #[test]
    fn a_config_flag_with_no_path_is_an_error_rather_than_a_silent_default() {
        assert!(parse_config_arg(&["--config".to_string()]).is_err());
    }

    #[test]
    fn an_unknown_option_is_refused() {
        assert!(parse_config_arg(&["--wat".to_string()]).is_err());
    }
}
