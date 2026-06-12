// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_settings;
mod credentials;
mod identities;
mod links;
mod logging;
mod plugin;
mod plugins;
mod prompts;
mod repo_settings;
mod sessions;
mod spawn;
mod status;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, LogicalSize, Manager, WindowEvent,
};
use tauri_plugin_positioner::{Position, WindowExt};

use plugin::{PluginRegistry, plugins_list_credential_types};
use plugins::{GitHubPlugin, github_list_issues, github_list_repos};

/// Records when the popover was auto-hidden on blur. A tray click that *caused*
/// that blur (clicking the icon while the window is open) lands here within a few
/// milliseconds, so we treat it as "close" instead of immediately reopening.
#[derive(Default)]
struct PopoverState {
    last_auto_hide: Mutex<Option<Instant>>,
}

/// Minimum popover size, in logical pixels. Mirrors `minWidth`/`minHeight` on the
/// `main` window in `tauri.conf.json`; used to clamp a restored size so a hand-edited
/// or stale `settings.json` can't shrink the popover into an unusable sliver.
const MIN_POPOVER_WIDTH: f64 = 480.0;
const MIN_POPOVER_HEIGHT: f64 = 360.0;

/// Minimum Settings-window size, in logical pixels. Mirrors `minWidth`/`minHeight`
/// on the `settings` window in `tauri.conf.json`; clamps a restored size so a
/// stale or hand-edited `settings.json` can't shrink it below usable.
const MIN_SETTINGS_WIDTH: f64 = 560.0;
const MIN_SETTINGS_HEIGHT: f64 = 400.0;

/// Apply the persisted popover size (if any) to the `main` window. Called in
/// `setup()` while the window is still hidden, so the first show already has the
/// user's chosen dimensions — no resize flash.
fn restore_popover_size(app: &tauri::AppHandle) {
    let Some(size) = app_settings::load().window else {
        return;
    };
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let width = size.width.max(MIN_POPOVER_WIDTH);
    let height = size.height.max(MIN_POPOVER_HEIGHT);
    if let Err(e) = window.set_size(LogicalSize::new(width, height)) {
        tracing::warn!(error = %e, "failed to restore popover size");
    }
}

/// Save the popover's current logical size to the global settings file. Called
/// from the blur handler (the popover always hides on blur), so it captures the
/// final size after a resize drag without writing on every drag frame.
fn persist_popover_size(window: &tauri::Window) {
    let Ok(physical) = window.inner_size() else {
        return;
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let logical = physical.to_logical::<f64>(scale);
    // Load-merge so we don't clobber other fields (e.g. the chosen theme).
    let mut settings = app_settings::load();
    settings.window = Some(app_settings::WindowSize {
        width: logical.width,
        height: logical.height,
    });
    if let Err(e) = app_settings::save(&settings) {
        tracing::warn!(error = %e, "failed to persist popover size");
    } else {
        tracing::debug!(
            width = logical.width,
            height = logical.height,
            "persisted popover size"
        );
    }
}

/// Apply the persisted Settings-window size (if any) to the `settings` window.
/// Called in `setup()` while the window is still hidden, so it opens at the
/// user's chosen dimensions with no resize flash on first show.
fn restore_settings_size(app: &tauri::AppHandle) {
    let Some(size) = app_settings::load().settings_window else {
        return;
    };
    let Some(window) = app.get_webview_window("settings") else {
        return;
    };
    let width = size.width.max(MIN_SETTINGS_WIDTH);
    let height = size.height.max(MIN_SETTINGS_HEIGHT);
    if let Err(e) = window.set_size(LogicalSize::new(width, height)) {
        tracing::warn!(error = %e, "failed to restore settings size");
    }
}

/// Save the Settings window's current logical size to the global settings file.
/// The Settings window stays open on blur (unlike the popover), so we capture its
/// size when it loses focus or is closed — both land after a resize drag finishes.
fn persist_settings_size(window: &tauri::Window) {
    let Ok(physical) = window.inner_size() else {
        return;
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let logical = physical.to_logical::<f64>(scale);
    // Load-merge so we don't clobber other fields (popover size, theme).
    let mut settings = app_settings::load();
    settings.settings_window = Some(app_settings::WindowSize {
        width: logical.width,
        height: logical.height,
    });
    if let Err(e) = app_settings::save(&settings) {
        tracing::warn!(error = %e, "failed to persist settings size");
    } else {
        tracing::debug!(
            width = logical.width,
            height = logical.height,
            "persisted settings size"
        );
    }
}

fn show_popover(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.move_window(Position::TrayCenter);
        let _ = window.show();
        let _ = window.set_focus();
        // Tell the UI it's being shown so it can refresh; the popover hides on
        // blur, so each show is a fresh open where sessions/PRs may have changed
        // (e.g. work happened or windows closed while it was hidden).
        let _ = window.emit("popover-shown", ());
    }
}

fn main() {
    // The app binary doubles as the Claude Code hook helper. When invoked as
    // `maiestro hook <state> --workspace <ws-id>` (from a spawned worktree's
    // .claude/settings.local.json), handle the hook and exit BEFORE booting the
    // tray app — otherwise every hook would launch a second mAIestro. This path
    // is short-lived and writes only a status file, so it skips logging setup.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("hook") {
        status::run_hook_cli(&args[2..]);
        return;
    }

    logging::init();
    tracing::info!("mAIestro starting");

    let registry = PluginRegistry::builder()
        .register(GitHubPlugin)
        .build();

    tauri::Builder::default()
        .manage(PopoverState::default())
        .manage(registry)
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Relaunch focuses the existing app instead of spawning a second.
            show_popover(app);
        }))
        .invoke_handler(tauri::generate_handler![
            credentials::credentials_set,
            credentials::credentials_get,
            credentials::credentials_delete,
            plugins_list_credential_types,
            repo_settings::repos_list,
            repo_settings::repo_settings_schema,
            repo_settings::repo_settings_get,
            repo_settings::repo_settings_set,
            repo_settings::repo_set_visibility,
            repo_settings::repo_scan_env_files,
            github_list_repos,
            github_list_issues,
            identities::identities_list,
            identities::identities_get_default,
            identities::identities_set_default,
            identities::identities_add,
            links::open_url,
            links::open_path,
            spawn::spawn_work,
            spawn::prepare_spawn,
            spawn::suggest_short_title,
            spawn::draft_spawn_preview,
            spawn::confirm_spawn,
            spawn::create_issue,
            spawn::create_issue_direct,
            spawn::create_issue_and_spawn,
            spawn::open_in_editor,
            spawn::open_repo_in_editor,
            spawn::teardown,
            spawn::open_accessibility_settings,
            spawn::session_pr,
            spawn::session_create_pr,
            spawn::session_pr_checks,
            spawn::session_work_state,
            spawn::session_merge_pr,
            sessions::sessions_list,
            sessions::session_set_visibility,
            status::sessions_status_list,
            logging::logs_read,
            logging::logs_reveal,
            status::clear_session_error,
            app_settings::app_settings_get_theme,
            app_settings::app_settings_set_theme,
        ])
        .setup(|app| {
            // Menu-bar-only: no dock icon on macOS.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Apply the user's persisted popover size before the first show, so
            // the popover opens at their chosen dimensions with no resize flash.
            restore_popover_size(app.handle());
            // Same for the Settings window, restored before its first show.
            restore_settings_size(app.handle());

            // Live per-session status: drop orphaned status files, then watch
            // ~/.maiestro/status/ and forward changes to the popover as
            // `session-status` events. The watcher must outlive setup(), so park
            // it in managed state (dropping it would stop the watch).
            status::sweep_stale();
            // Heal any worktree hooks still pointing at a now-stale binary path
            // (a torn-down/rebuilt spawner), so live status survives across
            // teardowns and `tauri dev` rebuilds. See spawn.rs / issue #35.
            spawn::reconcile_all_session_hooks();
            match status::start_watcher(app.handle().clone()) {
                Ok(watcher) => {
                    app.manage(Mutex::new(watcher));
                }
                Err(e) => tracing::error!(error = %e, "status watcher failed to start"),
            }

            let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let logs = MenuItem::with_id(app, "logs", "Show Logs", true, None::<&str>)?;
            let separator = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "Quit mAIestro", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&settings, &logs, &separator, &quit])?;

            TrayIconBuilder::with_id("main")
                // White brain mark rendered as a macOS template image, so the
                // system tints it for both light and dark menu-bar appearances.
                .icon(tauri::include_image!("icons/tray.png"))
                .icon_as_template(true)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "quit" => app.exit(0),
                    "settings" => {
                        if let Some(window) = app.get_webview_window("settings") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "logs" => {
                        if let Some(window) = app.get_webview_window("logs") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    let app = tray.app_handle();
                    // Cache the tray rectangle so the positioner can place the window.
                    tauri_plugin_positioner::on_tray_event(app, &event);

                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let Some(window) = app.get_webview_window("main") else {
                            return;
                        };
                        let visible = window.is_visible().unwrap_or(false);

                        let recently_auto_hidden = {
                            let state = app.state::<PopoverState>();
                            let mut last = state.last_auto_hide.lock().unwrap();
                            let recent = last
                                .map(|t| t.elapsed() < Duration::from_millis(300))
                                .unwrap_or(false);
                            *last = None;
                            recent
                        };

                        if visible {
                            let _ = window.hide();
                        } else if !recently_auto_hidden {
                            show_popover(app);
                        }
                        // else: this click's mousedown already blurred + hid the
                        // window, so leave it closed.
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| match window.label() {
            // The menu-bar popover auto-hides on blur; capture its (possibly
            // just-resized) size before hiding so it survives the next launch.
            "main" => {
                if let WindowEvent::Focused(false) = event {
                    let app = window.app_handle();
                    if let Some(state) = app.try_state::<PopoverState>() {
                        *state.last_auto_hide.lock().unwrap() = Some(Instant::now());
                    }
                    persist_popover_size(window);
                    let _ = window.hide();
                }
            }
            // The Settings window stays open on blur (it's a normal window), so
            // persist its size whenever it loses focus or is closed — either lands
            // after a resize drag finishes, without writing on every drag frame.
            "settings" => match event {
                WindowEvent::Focused(false) | WindowEvent::CloseRequested { .. } => {
                    persist_settings_size(window);
                }
                _ => {}
            },
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running mAIestro");
}
