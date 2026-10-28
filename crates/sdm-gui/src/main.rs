//! SDM desktop GUI (iced) — a clean, modern download manager that lives in the
//! system tray and catches download links from the clipboard and the browser.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::Duration;

use iced::widget::{
    button, canvas, column, container, mouse_area, row, scrollable, text, text_input, Space,
};
use iced::{
    window, Alignment, Background, Border, Color, Element, Length, Point, Shadow, Size,
    Subscription, Task, Theme,
};
use serde::{Deserialize, Serialize};
use tray_icon::menu::{CheckMenuItem, MenuId};

use sdm_core::{human_bytes, human_time, CatchRequest, Download, Manager, Segment, Status};

// ---- palette (modern dark) ---------------------------------------------
const BG: Color = Color { r: 0.059, g: 0.067, b: 0.082, a: 1.0 }; // #0f1115
const SURFACE: Color = Color { r: 0.094, g: 0.106, b: 0.133, a: 1.0 }; // #181b22
const SURFACE_SEL: Color = Color { r: 0.122, g: 0.137, b: 0.173, a: 1.0 }; // #1f232c
const BORDER: Color = Color { r: 0.149, g: 0.169, b: 0.208, a: 1.0 }; // #262b35
const TEXT: Color = Color { r: 0.910, g: 0.918, b: 0.929, a: 1.0 }; // #e8eaed
const MUTED: Color = Color { r: 0.541, g: 0.565, b: 0.612, a: 1.0 }; // #8a909c
const ACCENT: Color = Color { r: 0.302, g: 0.498, b: 1.0, a: 1.0 }; // #4d7fff
const ACCENT_HOVER: Color = Color { r: 0.416, g: 0.576, b: 1.0, a: 1.0 }; // #6a93ff
const TRACK: Color = Color { r: 0.137, g: 0.153, b: 0.184, a: 1.0 }; // #23272f
const SUCCESS: Color = Color { r: 0.208, g: 0.769, b: 0.420, a: 1.0 }; // #35c46b
const DANGER: Color = Color { r: 1.0, g: 0.361, b: 0.361, a: 1.0 }; // #ff5c5c

const PROJECT_URL: &str = "https://github.com/kevl0317/SDM-Segmented-Download-Manager";
const URL_INPUT: &str = "url-input";
const POPUP_INPUT: &str = "popup-filename";

pub fn main() -> iced::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if has("--register-host") {
        native_host::register();
        return Ok(());
    }
    if has("--unregister-host") {
        native_host::unregister();
        return Ok(());
    }
    if has("--native-host") || args.iter().any(|a| a.starts_with("chrome-extension://")) {
        native_host::run();
        return Ok(());
    }
    // The browser host starts us with --background to take a catch: stay in the
    // tray so only the "download this?" popup appears.
    let start_hidden = has("--background");
    iced::daemon(title, update, view)
        .subscription(subscription)
        .theme(|_, _| sdm_theme())
        .default_font(ui_font())
        .run_with(move || init(start_hidden))
}

#[derive(Debug, Clone)]
enum Message {
    // main window
    UrlChanged(String),
    Add,
    Select(usize),
    Resume,
    Pause,
    Remove,
    OpenFolder,
    // catches, and the popup that confirms them
    Caught(CatchRequest),
    ClipboardUrl(String),
    PopupFilename(String),
    PopupDownload,
    PopupCancel,
    // menus (the window's menu bar and the tray share one event stream)
    MenuItem(MenuId),
    AboutClose,
    OpenProject,
    // windows & keys
    MainHwnd(Option<isize>),
    CloseRequested(window::Id),
    KeyEscape(window::Id),
    KeyNew,
    TrayOpen,
    ExitNow,
    // runtime
    Tick,
    Finished,
}

/// Persisted preferences — the same file and keys the original app used, so an
/// upgrade keeps the user's choices. Keys we don't own are preserved on save.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    clipboard_monitor: bool,
    clipboard_all: bool,
    close_to_tray: bool,
    show_completed: bool,
    always_on_top: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            clipboard_monitor: true,
            clipboard_all: false,
            close_to_tray: true,
            show_completed: true,
            always_on_top: false,
        }
    }
}

/// The toggles that live in the menu bar.
#[derive(Debug, Clone, Copy)]
enum Pref {
    ShowCompleted,
    AlwaysOnTop,
    CatchClipboard,
    CatchAll,
    KeepInTray,
}

impl Pref {
    const ALL: [Pref; 5] = [
        Pref::ShowCompleted,
        Pref::AlwaysOnTop,
        Pref::CatchClipboard,
        Pref::CatchAll,
        Pref::KeepInTray,
    ];
}

impl Settings {
    fn path() -> PathBuf {
        sdm_core::config_dir().join("settings.json")
    }

    fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let path = Self::path();
        let mut all: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if let Ok(serde_json::Value::Object(mine)) = serde_json::to_value(self) {
            all.extend(mine);
        }
        if let Ok(json) = serde_json::to_string_pretty(&all) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    fn flag(&mut self, pref: Pref) -> &mut bool {
        match pref {
            Pref::ShowCompleted => &mut self.show_completed,
            Pref::AlwaysOnTop => &mut self.always_on_top,
            Pref::CatchClipboard => &mut self.clipboard_monitor,
            Pref::CatchAll => &mut self.clipboard_all,
            Pref::KeepInTray => &mut self.close_to_tray,
        }
    }
}

struct App {
    manager: Manager,
    settings: Settings,
    url_input: String,
    selected: Option<usize>,
    main_window: window::Id,
    reveal_main: bool,
    popup: Option<Popup>,
    about: Option<window::Id>,
    last_clip: Option<String>,
    tray: Option<Tray>,
    menu_bar: Option<MenuBar>,
}

/// The small "download this?" window shown for a caught link.
struct Popup {
    id: window::Id,
    url: String,
    filename: String,
    headers: Option<Vec<(String, String)>>,
}

struct Tray {
    _icon: tray_icon::TrayIcon, // kept alive for the app's lifetime
    open: MenuId,
    quit: MenuId,
}

/// The main window's native menu bar (dark-themed on Windows).
struct MenuBar {
    #[cfg_attr(not(windows), allow(dead_code))]
    menu: tray_icon::menu::Menu,
    new_download: MenuId,
    open_folder: MenuId,
    exit: MenuId,
    resume_all: MenuId,
    pause_all: MenuId,
    clear_completed: MenuId,
    show_completed: CheckMenuItem,
    always_on_top: CheckMenuItem,
    catch_clipboard: CheckMenuItem,
    catch_all: CheckMenuItem,
    keep_in_tray: CheckMenuItem,
    about: MenuId,
    project: MenuId,
}

impl MenuBar {
    fn check(&self, pref: Pref) -> &CheckMenuItem {
        match pref {
            Pref::ShowCompleted => &self.show_completed,
            Pref::AlwaysOnTop => &self.always_on_top,
            Pref::CatchClipboard => &self.catch_clipboard,
            Pref::CatchAll => &self.catch_all,
            Pref::KeepInTray => &self.keep_in_tray,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum MenuAction {
    Open,
    Quit,
    NewDownload,
    OpenFolder,
    ResumeAll,
    PauseAll,
    ClearCompleted,
    Pref(Pref),
    About,
    Project,
}

impl App {
    fn selected_download(&self) -> Option<&Download> {
        let id = self.selected?;
        self.manager.downloads.iter().find(|d| d.id() == id)
    }
}

fn init(start_hidden: bool) -> (App, Task<Message>) {
    let mut manager = Manager::new(None);
    manager.load_queue();
    let settings = Settings::load();
    let tray = build_tray();
    let menu_bar = build_menu_bar(&settings);
    let (main_window, opened) = window::open(main_settings());
    // Only stay hidden when there's a tray icon to come back from.
    let reveal_main = !(start_hidden && tray.is_some());
    let app = App {
        manager,
        settings,
        url_input: String::new(),
        selected: None,
        main_window,
        reveal_main,
        popup: None,
        about: None,
        last_clip: None,
        tray,
        menu_bar,
    };
    // Dark frame first, then attach the menu bar (MainHwnd) — which reveals it.
    let task = opened.then(|id| {
        darken(id).chain(window::run_with_handle(id, hwnd_of).map(Message::MainHwnd))
    });
    (app, task)
}

fn start_task(d: Download) -> Task<Message> {
    Task::perform(async move { d.run().await }, |_| Message::Finished)
}

fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::UrlChanged(s) => {
            app.url_input = s;
            Task::none()
        }
        Message::Add => {
            let url = app.url_input.trim().to_string();
            if url.is_empty() {
                return Task::none();
            }
            let d = app.manager.add(&url, None, None, 8, None);
            app.url_input.clear();
            app.selected = Some(d.id());
            start_task(d)
        }
        Message::Select(id) => {
            app.selected = Some(id);
            Task::none()
        }
        Message::Resume => {
            if let Some(d) = app.selected_download() {
                if matches!(d.status(), Status::Paused | Status::Error) {
                    let d = d.clone();
                    return start_task(d);
                }
            }
            Task::none()
        }
        Message::Pause => {
            if let Some(d) = app.selected_download() {
                d.pause();
            }
            Task::none()
        }
        Message::Remove => {
            if let Some(id) = app.selected {
                if let Some(d) = app.manager.downloads.iter().find(|x| x.id() == id).cloned() {
                    app.manager.remove(&d, false);
                    app.selected = None;
                }
            }
            Task::none()
        }
        Message::OpenFolder => {
            if let Some(d) = app.selected_download() {
                open_external(d.dest_dir());
            }
            Task::none()
        }
        Message::Caught(req) => {
            if req.url.is_empty() {
                return Task::none();
            }
            let filename = if req.filename.is_empty() {
                sdm_core::filename_from_url(&req.url)
            } else {
                sdm_core::sanitize_filename(&req.filename)
            };
            let headers = headers_for(&req);
            open_popup(app, req.url, filename, Some(headers))
        }
        Message::ClipboardUrl(url) => {
            let s = app.settings;
            let wanted = s.clipboard_monitor
                && (s.clipboard_all || sdm_core::extract_download_url(&url, false).is_some());
            if !wanted || app.last_clip.as_deref() == Some(url.as_str()) {
                return Task::none();
            }
            app.last_clip = Some(url.clone());
            let filename = sdm_core::filename_from_url(&url);
            open_popup(app, url, filename, None)
        }
        Message::PopupFilename(s) => {
            if let Some(p) = app.popup.as_mut() {
                p.filename = s;
            }
            Task::none()
        }
        Message::PopupDownload => {
            let Some(p) = app.popup.take() else {
                return Task::none();
            };
            let name = p.filename.trim();
            let d = app.manager.add(
                &p.url,
                None,
                (!name.is_empty()).then_some(name),
                8,
                p.headers.clone(),
            );
            app.selected = Some(d.id());
            // Only now does the full window come forward.
            Task::batch([start_task(d), window::close(p.id), show_main(app)])
        }
        Message::PopupCancel => close_popup(app),
        Message::MenuItem(id) => match classify(app, &id) {
            Some(action) => run_menu(app, action),
            None => Task::none(),
        },
        Message::AboutClose => close_about(app),
        Message::OpenProject => {
            open_external(PROJECT_URL);
            Task::none()
        }
        Message::MainHwnd(hwnd) => {
            attach_menu(app, hwnd);
            let level = window::change_level(app.main_window, level_for(app.settings.always_on_top));
            if app.reveal_main {
                level.chain(show_main(app))
            } else {
                level
            }
        }
        Message::CloseRequested(id) => {
            if app.popup.as_ref().map(|p| p.id) == Some(id) {
                close_popup(app)
            } else if app.about == Some(id) {
                close_about(app)
            } else if id == app.main_window {
                if app.settings.close_to_tray && app.tray.is_some() {
                    // Keep running in the tray so downloads and catching continue.
                    window::change_mode(id, window::Mode::Hidden)
                } else {
                    quit(app)
                }
            } else {
                window::close(id)
            }
        }
        Message::KeyEscape(id) => {
            if app.popup.as_ref().map(|p| p.id) == Some(id) {
                close_popup(app)
            } else if app.about == Some(id) {
                close_about(app)
            } else {
                Task::none()
            }
        }
        Message::KeyNew => new_download(app),
        Message::TrayOpen => show_main(app),
        Message::ExitNow => {
            app.tray = None; // remove the tray icon before the process goes away
            iced::exit()
        }
        Message::Tick => Task::none(),
        Message::Finished => {
            app.manager.save_queue();
            Task::none()
        }
    }
}

// ---- menus --------------------------------------------------------------

fn classify(app: &App, id: &MenuId) -> Option<MenuAction> {
    use MenuAction::*;
    if let Some(t) = &app.tray {
        if &t.open == id {
            return Some(Open);
        }
        if &t.quit == id {
            return Some(Quit);
        }
    }
    let m = app.menu_bar.as_ref()?;
    let items = [
        (&m.new_download, NewDownload),
        (&m.open_folder, OpenFolder),
        (&m.exit, Quit),
        (&m.resume_all, ResumeAll),
        (&m.pause_all, PauseAll),
        (&m.clear_completed, ClearCompleted),
        (&m.about, About),
        (&m.project, Project),
    ];
    if let Some((_, action)) = items.into_iter().find(|(mid, _)| *mid == id) {
        return Some(action);
    }
    // (the glob import above shadows the `Pref` enum with the variant)
    crate::Pref::ALL
        .into_iter()
        .find(|p| m.check(*p).id() == id)
        .map(MenuAction::Pref)
}

fn run_menu(app: &mut App, action: MenuAction) -> Task<Message> {
    match action {
        MenuAction::Open => show_main(app),
        MenuAction::Quit => quit(app),
        MenuAction::NewDownload => new_download(app),
        MenuAction::OpenFolder => {
            open_external(&app.manager.default_dir);
            Task::none()
        }
        MenuAction::ResumeAll => Task::batch(
            app.manager
                .downloads
                .iter()
                .filter(|d| matches!(d.status(), Status::Paused | Status::Error))
                .cloned()
                .map(start_task)
                .collect::<Vec<_>>(),
        ),
        MenuAction::PauseAll => {
            app.manager.pause_all();
            Task::none()
        }
        MenuAction::ClearCompleted => {
            let done: Vec<Download> = app
                .manager
                .downloads
                .iter()
                .filter(|d| d.status() == Status::Complete)
                .cloned()
                .collect();
            for d in &done {
                if app.selected == Some(d.id()) {
                    app.selected = None;
                }
                app.manager.remove(d, false);
            }
            Task::none()
        }
        MenuAction::Pref(pref) => {
            let on = flip(app, pref);
            match pref {
                Pref::AlwaysOnTop => window::change_level(app.main_window, level_for(on)),
                _ => Task::none(),
            }
        }
        MenuAction::About => open_about(app),
        MenuAction::Project => {
            open_external(PROJECT_URL);
            Task::none()
        }
    }
}

/// Flip a setting, mirror it on its menu check mark, and persist it.
fn flip(app: &mut App, pref: Pref) -> bool {
    let flag = app.settings.flag(pref);
    *flag = !*flag;
    let on = *flag;
    if let Some(m) = &app.menu_bar {
        m.check(pref).set_checked(on);
    }
    app.settings.save();
    on
}

fn build_menu_bar(s: &Settings) -> Option<MenuBar> {
    use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers};
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
    let ctrl_n = Accelerator::new(Some(Modifiers::CONTROL), Code::KeyN);
    let new_download = MenuItem::new("&New download", true, Some(ctrl_n));
    let open_folder = MenuItem::new("&Open downloads folder", true, None);
    let exit = MenuItem::new("E&xit", true, None);
    let resume_all = MenuItem::new("&Resume all", true, None);
    let pause_all = MenuItem::new("&Pause all", true, None);
    let clear_completed = MenuItem::new("&Clear completed", true, None);
    let show_completed =
        CheckMenuItem::new("Show &completed downloads", true, s.show_completed, None);
    let always_on_top = CheckMenuItem::new("Always on &top", true, s.always_on_top, None);
    let catch_clipboard =
        CheckMenuItem::new("Catch &copied links", true, s.clipboard_monitor, None);
    let catch_all = CheckMenuItem::new("Catch &all link types", true, s.clipboard_all, None);
    let keep_in_tray = CheckMenuItem::new(
        "Keep running in the &tray when closed",
        true,
        s.close_to_tray,
        None,
    );
    let about = MenuItem::new("&About SDM", true, None);
    let project = MenuItem::new("&Project page", true, None);
    let sep = PredefinedMenuItem::separator;

    let menu = Menu::new();
    menu.append(
        &Submenu::with_items("&File", true, &[&new_download, &open_folder, &sep(), &exit]).ok()?,
    )
    .ok()?;
    menu.append(
        &Submenu::with_items(
            "&Downloads",
            true,
            &[&resume_all, &pause_all, &sep(), &clear_completed],
        )
        .ok()?,
    )
    .ok()?;
    menu.append(&Submenu::with_items("&View", true, &[&show_completed, &always_on_top]).ok()?)
        .ok()?;
    menu.append(
        &Submenu::with_items(
            "&Options",
            true,
            &[&catch_clipboard, &catch_all, &sep(), &keep_in_tray],
        )
        .ok()?,
    )
    .ok()?;
    menu.append(&Submenu::with_items("&Help", true, &[&about, &project]).ok()?)
        .ok()?;

    Some(MenuBar {
        menu,
        new_download: new_download.id().clone(),
        open_folder: open_folder.id().clone(),
        exit: exit.id().clone(),
        resume_all: resume_all.id().clone(),
        pause_all: pause_all.id().clone(),
        clear_completed: clear_completed.id().clone(),
        show_completed,
        always_on_top,
        catch_clipboard,
        catch_all,
        keep_in_tray,
        about: about.id().clone(),
        project: project.id().clone(),
    })
}

#[cfg(windows)]
fn attach_menu(app: &App, hwnd: Option<isize>) {
    if let (Some(hwnd), Some(m)) = (hwnd, &app.menu_bar) {
        // SAFETY: the main window's live handle, used on its owning (UI) thread.
        unsafe {
            let _ = m
                .menu
                .init_for_hwnd_with_theme(hwnd, tray_icon::menu::MenuTheme::Dark);
        }
    }
}

#[cfg(not(windows))]
fn attach_menu(_app: &App, _hwnd: Option<isize>) {}

fn hwnd_of(handle: window::raw_window_handle::WindowHandle<'_>) -> Option<isize> {
    match handle.as_raw() {
        window::raw_window_handle::RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

// ---- windows ------------------------------------------------------------

fn open_popup(
    app: &mut App,
    url: String,
    filename: String,
    headers: Option<Vec<(String, String)>>,
) -> Task<Message> {
    // The filename field takes focus, so Enter downloads and Esc cancels.
    let focus = text_input::focus(text_input::Id::new(POPUP_INPUT));
    if let Some(p) = app.popup.as_mut() {
        // One popup at a time: retarget it at the newest catch.
        p.url = url;
        p.filename = filename;
        p.headers = headers;
        return window::gain_focus(p.id).chain(focus);
    }
    let (id, opened) = window::open(popup_settings());
    app.popup = Some(Popup {
        id,
        url,
        filename,
        headers,
    });
    opened.then(reveal).chain(focus)
}

fn close_popup(app: &mut App) -> Task<Message> {
    match app.popup.take() {
        Some(p) => window::close(p.id),
        None => Task::none(),
    }
}

fn open_about(app: &mut App) -> Task<Message> {
    if let Some(id) = app.about {
        return window::gain_focus(id);
    }
    let (id, opened) = window::open(about_settings());
    app.about = Some(id);
    opened.then(reveal)
}

fn close_about(app: &mut App) -> Task<Message> {
    match app.about.take() {
        Some(id) => window::close(id),
        None => Task::none(),
    }
}

fn show_main(app: &App) -> Task<Message> {
    let id = app.main_window;
    window::change_mode(id, window::Mode::Windowed).chain(window::gain_focus(id))
}

fn new_download(app: &App) -> Task<Message> {
    show_main(app).chain(text_input::focus(text_input::Id::new(URL_INPUT)))
}

fn quit(app: &mut App) -> Task<Message> {
    app.manager.pause_all();
    app.manager.save_queue();
    // Give paused workers a moment to checkpoint their sidecars before exiting.
    Task::perform(tokio::time::sleep(Duration::from_millis(700)), |_| {
        Message::ExitNow
    })
}

fn level_for(on_top: bool) -> window::Level {
    if on_top {
        window::Level::AlwaysOnTop
    } else {
        window::Level::Normal
    }
}

/// Windows open hidden; apply the dark frame first, then show, so there's no
/// light-title-bar flash.
fn reveal(id: window::Id) -> Task<Message> {
    darken(id)
        .chain(window::change_mode(id, window::Mode::Windowed))
        .chain(window::gain_focus(id))
}

fn darken(id: window::Id) -> Task<Message> {
    window::run_with_handle(id, set_dark_titlebar).discard()
}

fn main_settings() -> window::Settings {
    window::Settings {
        size: Size::new(760.0, 620.0),
        min_size: Some(Size::new(520.0, 380.0)),
        position: window::Position::Centered,
        visible: false,
        exit_on_close_request: false, // closing hides to the tray
        icon: window_icon(),
        ..window::Settings::default()
    }
}

fn popup_settings() -> window::Settings {
    window::Settings {
        size: Size::new(520.0, 200.0),
        position: window::Position::Centered,
        resizable: false,
        level: window::Level::AlwaysOnTop,
        visible: false,
        exit_on_close_request: false,
        icon: window_icon(),
        ..window::Settings::default()
    }
}

fn about_settings() -> window::Settings {
    window::Settings {
        size: Size::new(440.0, 214.0),
        position: window::Position::Centered,
        resizable: false,
        visible: false,
        exit_on_close_request: false,
        icon: window_icon(),
        ..window::Settings::default()
    }
}

fn window_icon() -> Option<window::Icon> {
    window::icon::from_rgba(logo_rgba(64), 64, 64).ok()
}

#[cfg(windows)]
fn set_dark_titlebar(handle: window::raw_window_handle::WindowHandle<'_>) {
    use window::raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    const DWMWA_USE_IMMERSIVE_DARK_MODE: i32 = 20;
    if let RawWindowHandle::Win32(h) = handle.as_raw() {
        let enabled: i32 = 1;
        unsafe {
            DwmSetWindowAttribute(
                h.hwnd.get(),
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &enabled as *const i32 as *const std::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            );
        }
    }
}

#[cfg(not(windows))]
fn set_dark_titlebar(_handle: window::raw_window_handle::WindowHandle<'_>) {}

// ---- tray & icon --------------------------------------------------------

fn build_tray() -> Option<Tray> {
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
    let open = MenuItem::new("Open SDM", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append(&open).ok()?;
    menu.append(&PredefinedMenuItem::separator()).ok()?;
    menu.append(&quit).ok()?;
    let icon = tray_icon::Icon::from_rgba(logo_rgba(32), 32, 32).ok()?;
    let tray = tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("SDM — Segmented Download Manager")
        .with_icon(icon)
        .build()
        .ok()?;
    Some(Tray {
        _icon: tray,
        open: open.id().clone(),
        quit: quit.id().clone(),
    })
}

/// The app mark: a blue rounded square with a white download arrow, drawn
/// with 4×4 supersampling so edges stay smooth at any size.
fn logo_rgba(n: u32) -> Vec<u8> {
    let s = n as f32;
    let radius = s * 0.22;
    let mid = s / 2.0;
    let in_square = |x: f32, y: f32| {
        let dx = x - x.clamp(radius, s - radius);
        let dy = y - y.clamp(radius, s - radius);
        dx * dx + dy * dy <= radius * radius
    };
    let in_arrow = |x: f32, y: f32| {
        let shaft = (x - mid).abs() <= s * 0.075 && (s * 0.20..=s * 0.55).contains(&y);
        let (top, apex, half) = (s * 0.42, s * 0.72, s * 0.24);
        let head = (top..=apex).contains(&y) && (x - mid).abs() <= half * (apex - y) / (apex - top);
        let tray = (s * 0.79..=s * 0.86).contains(&y) && (x - mid).abs() <= s * 0.28;
        shaft || head || tray
    };
    const SS: u32 = 4;
    let mut px = vec![0u8; (n * n * 4) as usize];
    for py in 0..n {
        for pxl in 0..n {
            let (mut cover, mut white) = (0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = pxl as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    if in_square(x, y) {
                        cover += 1;
                        if in_arrow(x, y) {
                            white += 1;
                        }
                    }
                }
            }
            let alpha = cover as f32 / (SS * SS) as f32;
            let w = if cover > 0 { white as f32 / cover as f32 } else { 0.0 };
            let t = py as f32 / s; // gentle top-to-bottom gradient
            let base = [
                91.0 * (1.0 - t) + 63.0 * t,
                140.0 * (1.0 - t) + 111.0 * t,
                255.0 * (1.0 - t) + 239.0 * t,
            ];
            let i = ((py * n + pxl) * 4) as usize;
            for c in 0..3 {
                px[i + c] = (base[c] * (1.0 - w) + 255.0 * w) as u8;
            }
            px[i + 3] = (alpha * 255.0) as u8;
        }
    }
    px
}

// ---- subscriptions --------------------------------------------------------

fn subscription(_app: &App) -> Subscription<Message> {
    Subscription::batch([
        iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick),
        Subscription::run(ipc_stream),
        Subscription::run(menu_stream),
        Subscription::run(clip_stream),
        window::close_requests().map(Message::CloseRequested),
        iced::event::listen_with(|event, _status, id| match event {
            iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                match key.as_ref() {
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape) => {
                        Some(Message::KeyEscape(id))
                    }
                    iced::keyboard::Key::Character("n") if modifiers.command() => {
                        Some(Message::KeyNew)
                    }
                    _ => None,
                }
            }
            _ => None,
        }),
    ])
}

// Runs the loopback IPC server and forwards each caught request into the app.
fn ipc_stream() -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(16, |mut output| async move {
        use iced::futures::SinkExt;
        let (tx, mut rx) = tokio::sync::mpsc::channel::<CatchRequest>(16);
        tokio::spawn(sdm_core::ipc::serve(tx));
        while let Some(req) = rx.recv().await {
            let _ = output.send(Message::Caught(req)).await;
        }
    })
}

// Menu clicks (menu bar and tray share muda's channel) and tray double-clicks.
fn menu_stream() -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(8, |mut output| async move {
        use iced::futures::SinkExt;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
        let menu_tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = tray_icon::menu::MenuEvent::receiver().recv() {
                if menu_tx.send(Message::MenuItem(ev.id)).is_err() {
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            while let Ok(ev) = tray_icon::TrayIconEvent::receiver().recv() {
                if matches!(ev, tray_icon::TrayIconEvent::DoubleClick { .. })
                    && tx.send(Message::TrayOpen).is_err()
                {
                    break;
                }
            }
        });
        while let Some(msg) = rx.recv().await {
            let _ = output.send(msg).await;
        }
    })
}

// Event-driven clipboard catching: the monitor wakes only on WM_CLIPBOARDUPDATE
// (AddClipboardFormatListener), so nothing is polled. Content already on the
// clipboard at startup is ignored — only new copies count. Links are filtered
// against the user's settings in `update`.
#[cfg(windows)]
fn clip_stream() -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(8, |mut output| async move {
        use iced::futures::SinkExt;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        std::thread::spawn(move || {
            let Ok(mut monitor) = clipboard_win::Monitor::new() else {
                return;
            };
            while let Ok(true) = monitor.recv() {
                // Let the copying app finish first — OLE apps set the data and then
                // flush it, and opening the clipboard in between makes *their* copy
                // fail — and fold a burst of updates from one copy into one read.
                std::thread::sleep(Duration::from_millis(120));
                while let Ok(true) = monitor.try_recv() {}
                let text = (0..3).find_map(|attempt| {
                    if attempt > 0 {
                        std::thread::sleep(Duration::from_millis(60));
                    }
                    clipboard_win::get_clipboard_string().ok()
                });
                if let Some(url) = text.and_then(|t| sdm_core::extract_download_url(&t, true)) {
                    if tx.send(url).is_err() {
                        break;
                    }
                }
            }
        });
        while let Some(url) = rx.recv().await {
            let _ = output.send(Message::ClipboardUrl(url)).await;
        }
    })
}

#[cfg(not(windows))]
fn clip_stream() -> impl iced::futures::Stream<Item = Message> {
    iced::futures::stream::empty()
}

// Turn a caught request's cookies / UA / referrer into request headers so
// authenticated (session) downloads work.
fn headers_for(req: &CatchRequest) -> Vec<(String, String)> {
    let mut h = sdm_core::default_headers();
    if !req.user_agent.is_empty() {
        for kv in h.iter_mut() {
            if kv.0.eq_ignore_ascii_case("user-agent") {
                kv.1 = req.user_agent.clone();
            }
        }
    }
    if !req.cookies.is_empty() {
        h.push(("Cookie".to_string(), req.cookies.clone()));
    }
    if !req.referrer.is_empty() {
        h.push(("Referer".to_string(), req.referrer.clone()));
    }
    h
}

fn open_external(target: impl AsRef<OsStr>) {
    let target = target.as_ref();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(target).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(target).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(target).spawn();
}

// ---- view ---------------------------------------------------------------

fn title(app: &App, id: window::Id) -> String {
    if app.popup.as_ref().map(|p| p.id) == Some(id) {
        "SDM — New download".to_string()
    } else if app.about == Some(id) {
        "About SDM".to_string()
    } else {
        "SDM".to_string()
    }
}

fn view(app: &App, id: window::Id) -> Element<'_, Message> {
    match &app.popup {
        Some(p) if p.id == id => return popup_view(p),
        _ => {}
    }
    if app.about == Some(id) {
        return about_view();
    }
    main_view(app)
}

fn main_view(app: &App) -> Element<'_, Message> {
    let active = app
        .manager
        .downloads
        .iter()
        .filter(|d| d.status() == Status::Running)
        .count();
    let total: u64 = app
        .manager
        .downloads
        .iter()
        .filter(|d| d.status() == Status::Running)
        .map(|d| d.speed())
        .sum();

    let header_right: Element<Message> = if active > 0 {
        text(format!("↓ {}/s", human_bytes(Some(total))))
            .size(13.0)
            .color(ACCENT)
            .into()
    } else {
        Space::with_width(Length::Fixed(0.0)).into()
    };

    let header = row![
        logo(20.0),
        text("SDM").size(20.0).font(semibold()),
        Space::with_width(Length::Fill),
        header_right,
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let add = row![
        text_input("Paste a link to download…", &app.url_input)
            .id(text_input::Id::new(URL_INPUT))
            .on_input(Message::UrlChanged)
            .on_submit(Message::Add)
            .padding([12.0, 14.0])
            .size(15.0)
            .style(input_style)
            .width(Length::Fill),
        button(text("Download").size(14.0))
            .on_press(Message::Add)
            .padding([12.0, 20.0])
            .style(accent_button),
    ]
    .spacing(10);

    let visible: Vec<&Download> = app
        .manager
        .downloads
        .iter()
        .filter(|d| app.settings.show_completed || d.status() != Status::Complete)
        .collect();

    let body: Element<Message> = if app.manager.downloads.is_empty() {
        empty_state("Paste a link above to start a download.")
    } else if visible.is_empty() {
        empty_state("Nothing in progress — completed downloads are hidden (View ▸ Show completed).")
    } else {
        let mut list = column![].spacing(10);
        for d in visible {
            list = list.push(download_card(app.selected == Some(d.id()), d));
        }
        scrollable(list).height(Length::Fill).into()
    };

    container(column![header, add, body].spacing(18))
        .padding(24)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn empty_state(message: &'static str) -> Element<'static, Message> {
    container(text(message).size(14.0).color(MUTED))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

fn popup_view(p: &Popup) -> Element<'_, Message> {
    let header = row![
        logo(20.0),
        text("New download").size(16.0).font(semibold()),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let name = text_input("File name", &p.filename)
        .id(text_input::Id::new(POPUP_INPUT))
        .on_input(Message::PopupFilename)
        .on_submit(Message::PopupDownload)
        .padding([10.0, 12.0])
        .size(14.0)
        .style(input_style);

    let actions = row![
        Space::with_width(Length::Fill),
        ghost("Cancel", Message::PopupCancel),
        button(text("Download").size(14.0))
            .on_press(Message::PopupDownload)
            .padding([10.0, 20.0])
            .style(accent_button),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    container(
        column![
            header,
            text(middle_ellipsis(&p.url, 72)).size(12.0).color(MUTED),
            name,
            actions,
        ]
        .spacing(14),
    )
    .padding(20)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn about_view() -> Element<'static, Message> {
    let header = row![
        logo(34.0),
        column![
            text("SDM").size(20.0).font(semibold()),
            text(format!("Version {}", env!("CARGO_PKG_VERSION")))
                .size(12.0)
                .color(MUTED),
        ]
        .spacing(2),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let blurb = text(
        "A fast segmented download manager. It splits each file across parallel \
         connections and catches download links from your clipboard and browser.",
    )
    .size(13.0);

    let actions = row![
        Space::with_width(Length::Fill),
        ghost("Project page", Message::OpenProject),
        button(text("Close").size(14.0))
            .on_press(Message::AboutClose)
            .padding([8.0, 18.0])
            .style(accent_button),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    container(column![header, blurb, actions].spacing(16))
        .padding(20)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn middle_ellipsis(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let keep = (max - 1) / 2;
    let head: String = s.chars().take(keep).collect();
    let tail: String = s.chars().skip(n - keep).collect();
    format!("{head}…{tail}")
}

fn download_card(selected: bool, d: &Download) -> Element<'static, Message> {
    let id = d.id();
    let status = d.status();
    let pct = d.progress();

    let right: Element<Message> = match status {
        Status::Complete => text("Done").size(13.0).color(SUCCESS).into(),
        Status::Error => text("Failed").size(13.0).color(DANGER).into(),
        Status::Running | Status::Paused => match pct {
            Some(p) => text(format!("{:.0}%", p * 100.0))
                .size(13.0)
                .color(if status == Status::Running { ACCENT } else { MUTED })
                .into(),
            None => Space::with_width(Length::Fixed(0.0)).into(),
        },
        _ => Space::with_width(Length::Fixed(0.0)).into(),
    };

    let name_row = row![
        text(d.filename()).size(15.0),
        Space::with_width(Length::Fill),
        right,
    ]
    .align_y(Alignment::Center);

    let bar = canvas(SegmentBar {
        segments: d.segments_snapshot(),
        size: d.size(),
        progress: pct,
        done: status == Status::Complete,
    })
    .width(Length::Fill)
    .height(Length::Fixed(8.0));

    let mut meta_row = row![text(meta_label(d)).size(12.0).color(MUTED)].align_y(Alignment::Center);
    if selected {
        meta_row = meta_row.push(Space::with_width(Length::Fill));
        match status {
            Status::Running | Status::Probing | Status::Queued => {
                meta_row = meta_row.push(ghost("Pause", Message::Pause));
            }
            Status::Paused | Status::Error => {
                meta_row = meta_row.push(ghost("Resume", Message::Resume));
            }
            Status::Complete => {}
        }
        meta_row = meta_row.push(ghost("Open", Message::OpenFolder));
        meta_row = meta_row.push(ghost("Remove", Message::Remove));
    }

    let card = container(column![name_row, bar, meta_row].spacing(11))
        .padding(16)
        .width(Length::Fill)
        .style(card_style(selected));

    mouse_area(card).on_press(Message::Select(id)).into()
}

fn ghost(label: &'static str, msg: Message) -> Element<'static, Message> {
    button(text(label).size(12.0))
        .on_press(msg)
        .padding([4.0, 10.0])
        .style(ghost_button)
        .into()
}

fn meta_label(d: &Download) -> String {
    match d.status() {
        Status::Running => {
            let base = format!(
                "{} / {} · {}/s",
                human_bytes(Some(d.downloaded())),
                human_bytes(d.size()),
                human_bytes(Some(d.speed()))
            );
            match d.eta() {
                Some(_) => format!("{base} · {} left", human_time(d.eta())),
                None => base,
            }
        }
        Status::Complete => format!("{} · saved", human_bytes(d.size())),
        Status::Paused => format!(
            "{} / {} · paused",
            human_bytes(Some(d.downloaded())),
            human_bytes(d.size())
        ),
        Status::Error => d.error().unwrap_or_else(|| "Download failed".to_string()),
        Status::Queued => "Queued".to_string(),
        Status::Probing => "Connecting…".to_string(),
    }
}

// ---- fonts & theme ------------------------------------------------------

// A UI font with full CJK coverage on Windows (filenames are often non-Latin);
// elsewhere iced's default, which falls back through the system fonts.
#[cfg(target_os = "windows")]
fn ui_font() -> iced::Font {
    iced::Font::with_name("Microsoft YaHei UI")
}
#[cfg(not(target_os = "windows"))]
fn ui_font() -> iced::Font {
    iced::Font::DEFAULT
}

fn semibold() -> iced::Font {
    let mut f = ui_font();
    f.weight = iced::font::Weight::Semibold;
    f
}

fn sdm_theme() -> Theme {
    let mut p = Theme::Dark.palette();
    p.background = BG;
    p.text = TEXT;
    p.primary = ACCENT;
    p.success = SUCCESS;
    p.danger = DANGER;
    Theme::custom("SDM".to_string(), p)
}

// ---- widget styles ------------------------------------------------------

fn card_style(selected: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        text_color: Some(TEXT),
        background: Some(Background::Color(if selected { SURFACE_SEL } else { SURFACE })),
        border: Border {
            color: if selected { ACCENT } else { BORDER },
            width: 1.0,
            radius: 12.0.into(),
        },
        shadow: Shadow::default(),
    }
}

fn input_style(_: &Theme, status: text_input::Status) -> text_input::Style {
    let border_color = match status {
        text_input::Status::Focused => ACCENT,
        _ => BORDER,
    };
    text_input::Style {
        background: Background::Color(SURFACE),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: 10.0.into(),
        },
        icon: MUTED,
        placeholder: MUTED,
        value: TEXT,
        selection: Color { a: 0.3, ..ACCENT },
    }
}

fn accent_button(_: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => ACCENT_HOVER,
        button::Status::Disabled => Color { a: 0.4, ..ACCENT },
        _ => ACCENT,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: Color::WHITE,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 10.0.into(),
        },
        shadow: Shadow::default(),
    }
}

fn ghost_button(_: &Theme, status: button::Status) -> button::Style {
    let (bg, tc) = match status {
        button::Status::Hovered => (Some(Background::Color(SURFACE_SEL)), TEXT),
        _ => (None, MUTED),
    };
    button::Style {
        background: bg,
        text_color: tc,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 8.0.into(),
        },
        shadow: Shadow::default(),
    }
}

// ---- native-messaging host (browser hand-off) --------------------------

mod native_host {
    use sdm_core::ipc::{self, CatchRequest};
    use std::io::{Read, Write};

    const EXTENSION_ID: &str = "mbaoofinhcijeneijhaglnjonhikbojj";

    /// Stdio loop: the browser frames each message as a 4-byte LE length + JSON.
    /// We parse it, forward it to the running GUI, and ack.
    pub fn run() {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let mut reader = stdin.lock();
        let mut writer = stdout.lock();
        loop {
            let mut len_buf = [0u8; 4];
            if reader.read_exact(&mut len_buf).is_err() {
                break;
            }
            let len = u32::from_le_bytes(len_buf) as usize;
            if len == 0 || len > (1 << 20) {
                break;
            }
            let mut msg = vec![0u8; len];
            if reader.read_exact(&mut msg).is_err() {
                break;
            }
            let ok = match serde_json::from_slice::<CatchRequest>(&msg) {
                Ok(req) if !req.url.is_empty() => forward_with_launch(&req).is_ok(),
                _ => false,
            };
            let resp = format!("{{\"ok\":{}}}", ok);
            let bytes = resp.as_bytes();
            if writer.write_all(&(bytes.len() as u32).to_le_bytes()).is_err() {
                break;
            }
            if writer.write_all(bytes).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    }

    fn forward_with_launch(req: &CatchRequest) -> std::io::Result<()> {
        if ipc::forward(req).is_ok() {
            return Ok(());
        }
        // Not running — launch the GUI and retry until its IPC server is up
        // (forward() re-reads the endpoint each try; a fresh GUI publishes a new one).
        launch_gui();
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if ipc::forward(req).is_ok() {
                return Ok(());
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "SDM unreachable",
        ))
    }

    /// Start the GUI in the tray (only the catch popup should appear) as an
    /// independent process: no inherited stdio (those are the browser's
    /// native-messaging pipes) and, on Windows, broken away from the browser's
    /// job object so SDM keeps running after the port closes.
    fn launch_gui() {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--background")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Spawning inherits every inheritable handle, and ours are the
            // browser's pipes — the GUI would hold them open. Stop that first.
            stop_std_handle_inheritance();
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
            let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
            cmd.creation_flags(base | CREATE_BREAKAWAY_FROM_JOB);
            if cmd.spawn().is_ok() {
                return;
            }
            cmd.creation_flags(base); // the job may forbid breakaway
        }
        let _ = cmd.spawn();
    }

    #[cfg(windows)]
    fn stop_std_handle_inheritance() {
        use windows_sys::Win32::Foundation::{
            SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
        };
        use windows_sys::Win32::System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        };
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            unsafe {
                let h = GetStdHandle(which);
                if h != 0 && h != INVALID_HANDLE_VALUE {
                    SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
    }

    #[cfg(windows)]
    pub fn register() {
        let exe = std::env::current_exe().unwrap_or_default();
        let exe_str = exe.to_string_lossy();
        let manifest_path = sdm_core::config_dir().join("com.sdm.catch.json");
        let manifest = format!(
            r#"{{"name":"com.sdm.catch","description":"SDM native messaging host","path":{:?},"type":"stdio","allowed_origins":["chrome-extension://{}/"]}}"#,
            exe_str, EXTENSION_ID
        );
        let _ = std::fs::write(&manifest_path, manifest);
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let mp = manifest_path.to_string_lossy().to_string();
        for base in [
            r"Software\Google\Chrome\NativeMessagingHosts\com.sdm.catch",
            r"Software\Microsoft\Edge\NativeMessagingHosts\com.sdm.catch",
        ] {
            if let Ok((key, _)) = hkcu.create_subkey(base) {
                let _ = key.set_value("", &mp);
            }
        }
        println!("Registered com.sdm.catch -> {}", manifest_path.display());
    }

    #[cfg(windows)]
    pub fn unregister() {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for base in [
            r"Software\Google\Chrome\NativeMessagingHosts\com.sdm.catch",
            r"Software\Microsoft\Edge\NativeMessagingHosts\com.sdm.catch",
        ] {
            let _ = hkcu.delete_subkey_all(base);
        }
        let _ = std::fs::remove_file(sdm_core::config_dir().join("com.sdm.catch.json"));
        println!("Unregistered com.sdm.catch");
    }

    #[cfg(not(windows))]
    pub fn register() {
        eprintln!("Native-host registration is currently implemented for Windows only.");
    }
    #[cfg(not(windows))]
    pub fn unregister() {}
}

// ---- canvas: logo mark & per-segment progress bar ----------------------

/// The app mark, drawn as vectors so it stays crisp at any size and matches
/// the window/tray icon.
struct Logo;

impl<Message> canvas::Program<Message> for Logo {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        use canvas::Path;
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let s = bounds.width.min(bounds.height);
        let mid = s / 2.0;
        let square = Path::rounded_rectangle(Point::ORIGIN, Size::new(s, s), (s * 0.22).into());
        frame.fill(&square, ACCENT);
        let shaft = Path::rectangle(
            Point::new(mid - s * 0.075, s * 0.20),
            Size::new(s * 0.15, s * 0.36),
        );
        let head = Path::new(|b| {
            b.move_to(Point::new(mid - s * 0.24, s * 0.42));
            b.line_to(Point::new(mid + s * 0.24, s * 0.42));
            b.line_to(Point::new(mid, s * 0.72));
            b.close();
        });
        let tray = Path::rectangle(
            Point::new(mid - s * 0.28, s * 0.79),
            Size::new(s * 0.56, s * 0.07),
        );
        for part in [&shaft, &head, &tray] {
            frame.fill(part, Color::WHITE);
        }
        vec![frame.into_geometry()]
    }
}

fn logo(size: f32) -> Element<'static, Message> {
    canvas(Logo)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into()
}

struct SegmentBar {
    segments: Vec<Segment>,
    size: Option<u64>,
    progress: Option<f64>,
    done: bool,
}

impl<Message> canvas::Program<Message> for SegmentBar {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);
        let gap = 2.0;

        match (self.size, self.segments.is_empty()) {
            (Some(size), false) if size > 0 => {
                let n = self.segments.len() as f32;
                let avail = (w - gap * (n - 1.0)).max(1.0);
                let mut x = 0.0;
                for s in &self.segments {
                    let sw = (avail * (s.length() as f32 / size as f32)).max(2.0);
                    frame.fill_rectangle(Point::new(x, 0.0), Size::new(sw, h), TRACK);
                    let frac = if s.length() > 0 {
                        s.downloaded as f32 / s.length() as f32
                    } else {
                        0.0
                    };
                    if frac > 0.0 {
                        let col = if s.done() { SUCCESS } else { ACCENT };
                        frame.fill_rectangle(Point::new(x, 0.0), Size::new(sw * frac, h), col);
                    }
                    x += sw + gap;
                }
            }
            _ => {
                frame.fill_rectangle(Point::new(0.0, 0.0), Size::new(w.max(1.0), h), TRACK);
                if let Some(p) = self.progress {
                    frame.fill_rectangle(
                        Point::new(0.0, 0.0),
                        Size::new((w * p as f32).max(0.0), h),
                        if self.done { SUCCESS } else { ACCENT },
                    );
                }
            }
        }
        vec![frame.into_geometry()]
    }
}
