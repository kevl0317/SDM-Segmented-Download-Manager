//! SDM desktop GUI (iced): the download queue, a live per-segment bar, and the
//! usual queue controls, wired to the segmented engine in `sdm-core`.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::Path;
use std::time::Duration;

use iced::widget::{
    button, canvas, column, container, mouse_area, progress_bar, row, scrollable, text, text_input,
    Space,
};
use iced::{Color, Element, Length, Size, Subscription, Task, Theme};

use sdm_core::{human_bytes, human_time, Download, Manager, Segment, Status};

pub fn main() -> iced::Result {
    iced::application("SDM — Segmented Download Manager", update, view)
        .subscription(subscription)
        .theme(|_| sdm_theme())
        .window_size(Size::new(1000.0, 640.0))
        .run_with(init)
}

#[derive(Debug, Clone)]
enum Message {
    UrlChanged(String),
    ConnsChanged(String),
    Add,
    Select(usize),
    Resume,
    Pause,
    Remove,
    OpenFolder,
    PauseAll,
    ClearCompleted,
    Tick,
    Finished,
}

struct App {
    manager: Manager,
    url_input: String,
    conns_input: String,
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
            conns_input: "8".into(),
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
        Message::ConnsChanged(s) => {
            if s.is_empty() || (s.chars().all(|c| c.is_ascii_digit()) && s.len() <= 2) {
                app.conns_input = s;
            }
            Task::none()
        }
        Message::Add => {
            let url = app.url_input.trim().to_string();
            if url.is_empty() {
                return Task::none();
            }
            let conns = app.conns_input.parse::<usize>().unwrap_or(8).clamp(1, 32);
            let d = app.manager.add(&url, None, None, conns, None);
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
        Message::PauseAll => {
            app.manager.pause_all();
            Task::none()
        }
        Message::ClearCompleted => {
            let done: Vec<Download> = app
                .manager
                .downloads
                .iter()
                .filter(|d| d.status() == Status::Complete)
                .cloned()
                .collect();
            for d in done {
                app.manager.remove(&d, false);
            }
            Task::none()
        }
        Message::Tick => Task::none(), // re-render reads live state from each Download
        Message::Finished => {
            app.manager.save_queue();
            Task::none()
        }
    }
}

fn subscription(_app: &App) -> Subscription<Message> {
    iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick)
}

fn view(app: &App) -> Element<'_, Message> {
    let add_bar = row![
        text_input("Paste a download URL…", &app.url_input)
            .on_input(Message::UrlChanged)
            .on_submit(Message::Add)
            .padding(8)
            .width(Length::Fill),
        text_input("conns", &app.conns_input)
            .on_input(Message::ConnsChanged)
            .padding(8)
            .width(Length::Fixed(64.0)),
        button(text("Add")).on_press(Message::Add).padding([8.0, 16.0]),
    ]
    .spacing(8);

    let has_sel = app.selected_download();
    let toolbar = row![
        button("Resume").on_press_maybe(has_sel.map(|_| Message::Resume)),
        button("Pause").on_press_maybe(has_sel.map(|_| Message::Pause)),
        button("Remove").on_press_maybe(has_sel.map(|_| Message::Remove)),
        button("Open folder").on_press_maybe(has_sel.map(|_| Message::OpenFolder)),
        Space::with_width(Length::Fill),
        button("Pause all").on_press(Message::PauseAll),
        button("Clear completed").on_press(Message::ClearCompleted),
    ]
    .spacing(6);

    let header = container(
        row![
            heading("Name", Length::FillPortion(3)),
            heading("Size", Length::Fixed(90.0)),
            heading("Progress", Length::Fixed(230.0)),
            heading("Speed", Length::Fixed(90.0)),
            heading("ETA", Length::Fixed(70.0)),
            heading("Status", Length::Fixed(110.0)),
        ]
        .spacing(8),
    )
    .padding([4.0, 8.0]);

    let mut list = column![].spacing(2);
    if app.manager.downloads.is_empty() {
        list = list.push(
            container(text("No downloads yet — paste a URL above and hit Add.").size(14.0))
                .padding(24),
        );
    }
    for d in &app.manager.downloads {
        list = list.push(download_row(app.selected == Some(d.id()), d));
    }

    container(
        column![
            add_bar,
            toolbar,
            header,
            container(scrollable(list).height(Length::Fill)).height(Length::Fill),
            details_pane(app),
            status_bar(app),
        ]
        .spacing(10)
        .padding(12),
    )
    .into()
}

fn heading(label: &'static str, width: Length) -> Element<'static, Message> {
    container(text(label).size(13.0)).width(width).into()
}

fn download_row(selected: bool, d: &Download) -> Element<'static, Message> {
    let id = d.id();
    let status = d.status();

    let progress: Element<Message> = match d.progress() {
        Some(p) => row![
            progress_bar(0.0..=1.0, p as f32)
                .width(Length::Fill)
                .height(Length::Fixed(14.0)),
            text(format!("{:>5.1}%", p * 100.0)).size(12.0),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into(),
        None => text(if status == Status::Running {
            "downloading…"
        } else {
            "—"
        })
        .size(12.0)
        .into(),
    };

    let speed = if status == Status::Running {
        format!("{}/s", human_bytes(Some(d.speed())))
    } else {
        String::new()
    };
    let eta = if status == Status::Running {
        human_time(d.eta())
    } else {
        String::new()
    };

    let r = row![
        container(text(d.filename()).size(13.0)).width(Length::FillPortion(3)),
        container(text(human_bytes(d.size())).size(13.0)).width(Length::Fixed(90.0)),
        container(progress).width(Length::Fixed(230.0)),
        container(text(speed).size(13.0)).width(Length::Fixed(90.0)),
        container(text(eta).size(13.0)).width(Length::Fixed(70.0)),
        container(text(status_text(status)).size(13.0)).width(Length::Fixed(110.0)),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);

    mouse_area(
        container(r)
            .padding([6.0, 8.0])
            .width(Length::Fill)
            .style(move |theme: &Theme| {
                let p = theme.extended_palette();
                if selected {
                    container::Style {
                        background: Some(p.primary.weak.color.into()),
                        ..container::Style::default()
                    }
                } else {
                    container::Style::default()
                }
            }),
    )
    .on_press(Message::Select(id))
    .into()
}

fn details_pane(app: &App) -> Element<'_, Message> {
    let content: Element<Message> = match app.selected_download() {
        None => text("Select a download to see its segments")
            .size(13.0)
            .into(),
        Some(d) => {
            let bar = canvas(SegmentBar {
                segments: d.segments_snapshot(),
                size: d.size(),
                progress: d.progress(),
            })
            .width(Length::Fill)
            .height(Length::Fixed(30.0));
            column![
                text(d.filename()).size(15.0),
                bar,
                text(detail_meta(d)).size(13.0),
                text(format!("{}  →  {}", d.url(), d.output_path().display())).size(12.0),
            ]
            .spacing(8)
            .into()
        }
    };
    container(content)
        .padding(14)
        .width(Length::Fill)
        .style(card_style)
        .into()
}

fn status_bar(app: &App) -> Element<'_, Message> {
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
    let txt = if active > 0 {
        format!("{active} downloading   •   {}/s total", human_bytes(Some(total)))
    } else {
        "Ready".to_string()
    };
    container(text(txt).size(12.0))
        .padding([6.0, 10.0])
        .width(Length::Fill)
        .into()
}

fn detail_meta(d: &Download) -> String {
    let mut parts = vec![
        status_text(d.status()).to_string(),
        format!(
            "{} / {}",
            human_bytes(Some(d.downloaded())),
            human_bytes(d.size())
        ),
    ];
    if d.status() == Status::Running {
        parts.push(format!("{}/s", human_bytes(Some(d.speed()))));
        parts.push(format!("ETA {}", human_time(d.eta())));
    }
    parts.push(format!("{} connection(s)", d.segment_count().max(1)));
    parts.push(
        if d.supports_ranges() {
            "resumable"
        } else {
            "no range support"
        }
        .to_string(),
    );
    if let Some(e) = d.error() {
        parts.push(format!("— {e}"));
    }
    parts.join("   •   ")
}

fn status_text(s: Status) -> &'static str {
    match s {
        Status::Queued => "Queued",
        Status::Probing => "Connecting…",
        Status::Running => "Downloading",
        Status::Paused => "Paused",
        Status::Complete => "Completed",
        Status::Error => "Error",
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

fn sdm_theme() -> Theme {
    let mut p = Theme::Dark.palette();
    p.background = Color::from_rgb8(0x1e, 0x20, 0x24);
    p.primary = Color::from_rgb8(0x2f, 0x81, 0xf7);
    p.success = Color::from_rgb8(0x3f, 0xb9, 0x50);
    p.danger = Color::from_rgb8(0xe5, 0x48, 0x4d);
    Theme::custom("SDM".to_string(), p)
}

fn card_style(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(p.background.weak.color.into()),
        border: iced::Border {
            color: p.background.strong.color,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

// ---- per-segment progress bar (canvas) ---------------------------------

struct SegmentBar {
    segments: Vec<Segment>,
    size: Option<u64>,
    progress: Option<f64>,
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
        let pad = 4.0;
        let gap = 2.0;
        let track = Color::from_rgb8(0x33, 0x38, 0x3f);
        let accent = Color::from_rgb8(0x2f, 0x81, 0xf7);
        let success = Color::from_rgb8(0x3f, 0xb9, 0x50);

        match (self.size, self.segments.is_empty()) {
            (Some(size), false) if size > 0 => {
                let n = self.segments.len() as f32;
                let avail = (w - 2.0 * pad - gap * (n - 1.0)).max(1.0);
                let mut x = pad;
                for s in &self.segments {
                    let sw = (avail * (s.length() as f32 / size as f32)).max(3.0);
                    frame.fill_rectangle(Point::new(x, pad), Size::new(sw, h - 2.0 * pad), track);
                    let frac = if s.length() > 0 {
                        s.downloaded as f32 / s.length() as f32
                    } else {
                        0.0
                    };
                    if frac > 0.0 {
                        let col = if s.done() { success } else { accent };
                        frame.fill_rectangle(
                            Point::new(x, pad),
                            Size::new(sw * frac, h - 2.0 * pad),
                            col,
                        );
                    }
                    x += sw + gap;
                }
            }
            _ => {
                frame.fill_rectangle(
                    Point::new(pad, pad),
                    Size::new((w - 2.0 * pad).max(1.0), h - 2.0 * pad),
                    track,
                );
                if let Some(p) = self.progress {
                    frame.fill_rectangle(
                        Point::new(pad, pad),
                        Size::new(((w - 2.0 * pad) * p as f32).max(0.0), h - 2.0 * pad),
                        accent,
                    );
                }
            }
        }
        vec![frame.into_geometry()]
    }
}
