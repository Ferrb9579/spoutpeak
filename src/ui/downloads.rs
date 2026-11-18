//! The Downloads page: progress of running downloads and the saved files.

use super::motion;
use super::widgets::*;
use crate::downloads::{self, Download, DownloadStatus};
use crate::streaming::Swarm;
use crate::state::App;
use crate::theme::*;
use crate::util::{format_bytes, format_speed, format_time};
use incular::prelude::*;
use std::rc::Rc;

const ROW_HEIGHT: f32 = 128.0;
/// Secondary lines on a row: brighter than `MUTED`, since the row sits on
/// glass over whatever colours the ambient backdrop has.
const DETAIL: Color = Color::rgba(205, 205, 205, 255);

fn progress_bar(fraction: Option<f32>, color: Color) -> Widget {
    LayoutBuilder::new(move |_, constraints| {
        let width = constraints.max_width();
        let filled = fraction.map_or(width * 0.12, |f| width * f);
        Container::new()
            .width(width)
            .height(5.0)
            .radius(2.5)
            .color(GLASS_HI)
            .alignment(Alignment::CENTER_LEFT)
            .child(Container::new().width(filled.max(0.0)).height(5.0).radius(2.5).color(color))
            .into()
    })
    .into()
}

fn status_line(item: &Download) -> (String, Color) {
    let size = match item.total {
        Some(total) => format!("{} of {}", format_bytes(item.received), format_bytes(total)),
        None => format_bytes(item.received),
    };
    match &item.status {
        DownloadStatus::Preparing => ("Preparing…".to_owned(), DETAIL),
        DownloadStatus::Converting => {
            let mut parts = vec![format!("Converting to {}", item.format.label())];
            parts.push(format!("{:.0}%", item.converted * 100.0));
            if let Some(encoder) = item.encoder {
                parts.push(encoder.to_owned());
            }
            (parts.join("  ·  "), DETAIL)
        }
        DownloadStatus::Downloading => {
            let mut parts = vec![size];
            if let Some(fraction) = item.fraction() {
                parts.push(format!("{:.0}%", fraction * 100.0));
            }
            if item.speed > 0.0 {
                parts.push(format_speed(item.speed));
                if let Some(total) = item.total.filter(|t| *t > item.received) {
                    let left = (total - item.received) as f64 / item.speed;
                    if left < 100.0 * 3600.0 {
                        parts.push(format!("{} left", format_time(left)));
                    }
                }
            }
            (parts.join("  ·  "), DETAIL)
        }
        DownloadStatus::Done => (
            format!("Saved  ·  {}", format_bytes(item.total.unwrap_or(item.received))),
            GOOD,
        ),
        DownloadStatus::Failed(reason) => (format!("{reason}  ·  {size} saved"), DANGER),
        DownloadStatus::Cancelled => (format!("Cancelled  ·  {size} saved"), MUTED),
        DownloadStatus::Paused => {
            let mut parts = vec![format!("Paused  ·  {size}")];
            if let Some(fraction) = item.fraction() {
                parts.push(format!("{:.0}%", fraction * 100.0));
            }
            (parts.join("  ·  "), DETAIL)
        }
    }
}

/// Who a torrent download is connected to, e.g.
/// `14 peers (5 seeds)  ·  3 connecting  ·  211 found  ·  limit 200`.
fn swarm_line(swarm: &Swarm) -> String {
    let plural = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut parts = Vec::new();
    if swarm.peers == 0 {
        parts.push("No peers connected yet".to_owned());
    } else {
        parts.push(format!(
            "{} ({})",
            plural(swarm.peers, "peer", "peers"),
            plural(swarm.seeders, "seed", "seeds")
        ));
    }
    if swarm.connecting > 0 {
        parts.push(format!("{} connecting", swarm.connecting));
    }
    parts.push(format!("{} found", swarm.discovered));
    if let Some(limit) = swarm.limit {
        parts.push(format!("limit {limit}"));
    }
    parts.join("  ·  ")
}

/// A labelled-by-tooltip icon button.
fn action(glyph: Glyph, hint: &str, tone: Tone, on_click: impl Fn() + 'static) -> Widget {
    super::tooltip::tooltip(hint, small_icon_button(glyph, None, tone, on_click).accessibility_label(hint))
}

fn row_actions(app: &App, item: &Download) -> Vec<Widget> {
    let id = item.id;
    let call = |call: fn(&App, u64)| {
        let app = app.clone();
        move || call(&app, id)
    };
    let mut actions = Vec::new();
    match item.status {
        DownloadStatus::Preparing | DownloadStatus::Downloading => {
            // Plays from the source while the download keeps going (a torrent
            // shares its pieces between the two).
            actions.push(super::tooltip::tooltip("Play now, while it keeps downloading",
                small_icon_button(icons::play(), Some("Play"), Tone::Primary, call(|app, id| app.watch_download(id)))));
            actions.push(hgap(6.0));
            actions.push(small_icon_button(icons::pause(), Some("Pause"), Tone::Neutral, call(|app, id| app.pause_download(id))));
            actions.push(hgap(6.0));
            actions.push(action(icons::x(), "Cancel and delete the partial file", Tone::Danger,
                call(|app, id| app.delete_download(id))));
        }
        DownloadStatus::Converting => {
            actions.push(action(icons::x(), "Stop converting (Resume converts again)", Tone::Neutral,
                call(|app, id| app.cancel_download(id))));
        }
        DownloadStatus::Done => {
            actions.push(small_icon_button(icons::play(), Some("Play"), Tone::Primary, call(|app, id| app.play_download(id))));
            actions.push(hgap(6.0));
            actions.push(action(icons::folder_open(), "Show in folder", Tone::Neutral, call(|app, id| app.reveal_download(id))));
            actions.push(hgap(6.0));
            actions.push(action(icons::trash_2(), "Delete the file", Tone::Danger, call(|app, id| app.delete_download(id))));
        }
        DownloadStatus::Paused | DownloadStatus::Failed(_) | DownloadStatus::Cancelled => {
            actions.push(small_icon_button(icons::play(), Some("Resume"), Tone::Primary, call(|app, id| app.retry_download(id))));
            actions.push(hgap(6.0));
            actions.push(action(icons::trash_2(), "Delete the partial file", Tone::Danger, call(|app, id| app.delete_download(id))));
        }
    }
    actions
}

fn download_row(app: &App, item: &Download) -> Widget {
    let (status, status_color) = status_line(item);
    let mut info: Vec<Widget> = vec![
        line(item.title.clone(), 14.5, FontWeight::W600, TEXT),
        gap(3.0),
        line(item.source.clone(), 12.0, FontWeight::W400, MUTED),
        gap(10.0),
    ];
    if item.status.is_active() || item.status == DownloadStatus::Paused {
        let color = if item.status == DownloadStatus::Paused { MUTED } else { ACCENT };
        info.push(progress_bar(item.fraction(), color));
        info.push(gap(8.0));
    }
    info.push(line(status, 12.0, FontWeight::W500, status_color));
    if let (DownloadStatus::Downloading, Some(swarm)) = (&item.status, &item.swarm) {
        info.push(gap(4.0));
        info.push(line(swarm_line(swarm), 12.0, FontWeight::W400, MUTED));
    }

    Padding::new(
        EdgeInsets::only(0.0, 0.0, 0.0, 12.0),
        Container::new()
            .height(ROW_HEIGHT - 12.0)
            .padding(EdgeInsets::symmetric(16.0, 0.0))
            .alignment(Alignment::CENTER_LEFT)
            .decoration(
                BoxDecoration::new()
                    .color(GLASS_TINT)
                    .border_radius(BorderRadius::all(Radius::circular(16.0))),
            )
            .child(
                hrow(vec![
                    net_image(item.poster.as_deref(), 52.0, 76.0, 8.0, 160, &item.title),
                    hgap(16.0),
                    Expanded::new(col(info)).into(),
                    hgap(16.0),
                    row(row_actions(app, item)).into(),
                ])
                .cross_axis_alignment(CrossAxisAlignment::Center),
            ),
    )
    .into()
}

pub fn screen(app: &App) -> Widget {
    let items: Rc<Vec<Download>> = Rc::new(app.downloads.read(|list| list.clone()));
    let open = app.clone();
    let folder = downloads::folder().to_string_lossy().into_owned();
    let running = items.iter().any(|d| matches!(d.status, DownloadStatus::Preparing | DownloadStatus::Downloading));
    let stopped = items.iter().any(|d| d.status.can_resume());
    let finished = items.iter().any(|d| d.status == DownloadStatus::Done);
    let mut trailing: Vec<Widget> = Vec::new();
    if running {
        let app = app.clone();
        trailing.push(small_icon_button(icons::pause(), Some("Pause all"), Tone::Neutral, move || app.pause_all_downloads()));
        trailing.push(hgap(8.0));
    }
    if stopped {
        let app = app.clone();
        trailing.push(small_icon_button(icons::play(), Some("Resume all"), Tone::Neutral, move || app.resume_all_downloads()));
        trailing.push(hgap(8.0));
    }
    if finished {
        let app = app.clone();
        trailing.push(super::tooltip::tooltip("Remove finished downloads from this list. The files stay.",
            small_icon_button(icons::check(), Some("Clear finished"), Tone::Neutral, move || app.clear_finished_downloads())));
        trailing.push(hgap(16.0));
    }
    trailing.extend([
        // On its own glass: it sits straight on the ambient backdrop, which
        // can be as light as the text.
        glass(13.0)
            .padding(EdgeInsets::symmetric(11.0, 6.0))
            .child(line(folder, 12.0, FontWeight::W400, DETAIL))
            .into(),
        hgap(12.0),
        small_icon_button(icons::folder_open(), Some("Open folder"), Tone::Neutral, move || {
            open.open_download_folder()
        }),
    ]);
    let body: Widget = if items.is_empty() {
        Padding::new(
            PAGE_PADDING,
            notice(
                "Nothing downloaded yet. Open a title and use the download button next to a stream.",
                MUTED,
            ),
        )
        .into()
    } else {
        let controller = app.controllers.downloads.clone();
        let app = app.clone();
        smooth(
            &controller,
            ListView::builder(items.len(), move |index| download_row(&app, &items[index]))
                .item_extent(ROW_HEIGHT)
                .padding(PAGE_PADDING)
                .physics(motion::APP_SCROLLED)
                .controller(controller.clone()),
        )
    };
    page("Downloads", trailing, body)
}
