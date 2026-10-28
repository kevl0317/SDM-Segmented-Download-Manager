//! SDM desktop GUI (iced) — a clean, modern, card-based download manager.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::Path;
use std::time::Duration;

use iced::widget::{
    button, canvas, column, container, mouse_area, row, scrollable, text, text_input, Space,
};
use iced::{
    Alignment, Background, Border, Color, Element, Length, Shadow, Size, Subscription, Task, Theme,
};

use sdm_core::{human_bytes, human_time, Download, Manager, Segment, Status};

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

pub fn main() -> iced::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--register-host") {
        native_host::register();
        return Ok(());
    }
    if args.iter().any(|a| a == "--unregister-host") {
        native_host::unregister();
        return Ok(());
    }
    if args
        .iter()
        .any(|a| a == "--native-host" || a.starts_with("chrome-extension://"))
    {
        native_host::run();
        return Ok(());
    }
    apply_dark_titlebar_async();
    iced::application("SDM", update, view)
        .subscription(subscription)
        .theme(|_| sdm_theme())
        .default_font(ui_font())
        .window_size(Size::new(760.0, 620.0))
        .run_with(init)
}

#[derive(Debug, Clone)]
enum Message {
    UrlChanged(String),
    Add,
    Select(usize),
    Resume,
    Pause,
    Remove,
    OpenFolder,
    Caught(sdm_core::CatchRequest),
    Tick,
    Finished,
}

struct App {
    manager: Manager,
    url_input: String,
    selected: Option<usize>,
}

impl App {
    fn selected_download(&self) -> Option<&Download> {
        let id = self.selected?;
        self.manager.downloads.iter().find(|d| d.id() == id)
    }
}

fn init() -> (App, Task<Message>) {
    let mut manager = Manager::new(None);
    manager.load_queue();
    (
        App {
            manager,
            url_input: String::new(),
            selected: None,
        },
        Task::none(),
    )
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
                open_folder(d.dest_dir());
            }
            Task::none()
        }
        Message::Caught(req) => {
            if req.url.is_empty() {
                return Task::none();
            }
            let filename = if req.filename.is_empty() {
                None
            } else {
                Some(req.filename.as_str())
            };
            let headers = headers_for(&req);
            let d = app.manager.add(&req.url, None, filename, 8, Some(headers));
            app.selected = Some(d.id());
            start_task(d)
        }
        Message::Tick => Task::none(),
        Message::Finished => {
            app.manager.save_queue();
            Task::none()
        }
    }
}

fn subscription(_app: &App) -> Subscription<Message> {
    Subscription::batch([
        iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick),
        Subscription::run(ipc_stream),
    ])
}

// Runs the loopback IPC server and forwards each caught request into the app.
fn ipc_stream() -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(16, |mut output| async move {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<sdm_core::CatchRequest>(16);
        tokio::spawn(sdm_core::ipc::serve(tx));
        while let Some(req) = rx.recv().await {
            use iced::futures::SinkExt;
            let _ = output.send(Message::Caught(req)).await;
        }
    })
}

// Turn a caught request's cookies / UA / referrer into request headers so
// authenticated (session) downloads work.
fn headers_for(req: &sdm_core::CatchRequest) -> Vec<(String, String)> {
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

// ---- view ---------------------------------------------------------------

fn view(app: &App) -> Element<'_, Message> {
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

    let logo = container(Space::new(0, 0))
        .width(Length::Fixed(18.0))
        .height(Length::Fixed(18.0))
        .style(|_: &Theme| container::Style {
            background: Some(Background::Color(ACCENT)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 5.0.into(),
            },
            ..container::Style::default()
        });

    let header_right: Element<Message> = if active > 0 {
        text(format!("↓ {}/s", human_bytes(Some(total))))
            .size(13.0)
            .color(ACCENT)
            .into()
    } else {
        Space::with_width(Length::Fixed(0.0)).into()
    };

    let header = row![
        logo,
        text("SDM").size(20.0).font(semibold()),
        Space::with_width(Length::Fill),
        header_right,
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let add = row![
        text_input("Paste a link to download…", &app.url_input)
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

    let body: Element<Message> = if app.manager.downloads.is_empty() {
        container(
            text("Paste a link above to start a download.")
                .size(14.0)
                .color(MUTED),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
    } else {
        let mut list = column![].spacing(10);
        for d in &app.manager.downloads {
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

fn open_folder(dir: &Path) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

// ---- fonts & theme ------------------------------------------------------

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

// Windows draws the title bar itself; ask DWM to make it dark so the native
// frame matches the app. iced doesn't expose the raw window handle, so we find
// the window by title shortly after it opens and set the attribute.
#[cfg(windows)]
fn apply_dark_titlebar_async() {
    use std::ffi::c_void;
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetWindowPos, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SWP_NOZORDER,
    };
    const DWMWA_USE_IMMERSIVE_DARK_MODE: i32 = 20;
    std::thread::spawn(|| {
        let title: Vec<u16> = "SDM\0".encode_utf16().collect();
        for _ in 0..100 {
            let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
            if hwnd != 0 {
                let enabled: i32 = 1;
                unsafe {
                    DwmSetWindowAttribute(
                        hwnd,
                        DWMWA_USE_IMMERSIVE_DARK_MODE,
                        &enabled as *const i32 as *const c_void,
                        std::mem::size_of::<i32>() as u32,
                    );
                    SetWindowPos(
                        hwnd,
                        0,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                    );
                }
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
}
#[cfg(not(windows))]
fn apply_dark_titlebar_async() {}

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

    /// Start the GUI as an independent process: no inherited stdio (those are the
    /// browser's native-messaging pipes) and, on Windows, broken away from the
    /// browser's job object so SDM keeps running after the port closes.
    fn launch_gui() {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.stdin(std::process::Stdio::null())
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

// ---- per-segment progress bar (canvas) ---------------------------------

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
        use iced::Point;
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
