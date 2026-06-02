// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod credentials;
mod identities;
mod links;
mod plugin;
mod plugins;
mod repo_settings;
mod sessions;
mod spawn;
mod status;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WindowEvent,
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
    // tray app — otherwise every hook would launch a second mAIestro.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("hook") {
        status::run_hook_cli(&args[2..]);
        return;
    }

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
            repo_settings::repo_settings_get,
            repo_settings::repo_settings_set,
            repo_settings::repo_set_visibility,
            repo_settings::repo_scan_env_files,
            github_list_repos,
            github_list_issues,
            identities::identities_list,
            identities::identities_get_default,
            identities::identities_set_default,
            links::open_url,
            links::open_path,
            spawn::spawn_work,
            spawn::prepare_spawn,
            spawn::draft_spawn_preview,
            spawn::confirm_spawn,
            spawn::create_issue,
            spawn::create_issue_direct,
            spawn::create_issue_and_spawn,
            spawn::open_in_editor,
            spawn::teardown,
            spawn::open_accessibility_settings,
            spawn::session_pr,
            spawn::session_create_pr,
            spawn::session_pr_checks,
            spawn::session_merge_pr,
            sessions::sessions_list,
            sessions::session_set_visibility,
            status::sessions_status_list,
        ])
        .setup(|app| {
            // Menu-bar-only: no dock icon on macOS.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Live per-session status: drop orphaned status files, then watch
            // ~/.maiestro/status/ and forward changes to the popover as
            // `session-status` events. The watcher must outlive setup(), so park
            // it in managed state (dropping it would stop the watch).
            status::sweep_stale();
            match status::start_watcher(app.handle().clone()) {
                Ok(watcher) => {
                    app.manage(Mutex::new(watcher));
                }
                Err(e) => eprintln!("status watcher failed to start: {e}"),
            }

            let quit = MenuItem::with_id(app, "quit", "Quit mAIestro", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&quit])?;

            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id().as_ref() == "quit" {
                        app.exit(0);
                    }
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
        .on_window_event(|window, event| {
            // Only the menu-bar popover auto-hides on blur; the Settings window
            // is a normal window and must stay open when you click elsewhere.
            if window.label() != "main" {
                return;
            }
            // Hide (don't quit) when the popover loses focus, so clicking away
            // dismisses it like a normal menu-bar dropdown.
            if let WindowEvent::Focused(false) = event {
                let app = window.app_handle();
                if let Some(state) = app.try_state::<PopoverState>() {
                    *state.last_auto_hide.lock().unwrap() = Some(Instant::now());
                }
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running mAIestro");
}
