//! The Downloads page: progress of running downloads and the saved files.

use super::motion;
use super::widgets::*;
use crate::downloads::{self, Download, DownloadStatus};
use crate::state::App;
use crate::theme::*;
use crate::util::{format_bytes, format_speed};
use incular::prelude::*;
use std::rc::Rc;

const ROW_HEIGHT: f32 = 112.0;

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
        DownloadStatus::Preparing => ("Preparing…".to_owned(), MUTED),
        DownloadStatus::Converting => {
            let mut parts = vec![format!("Converting to {}", item.format.label())];
            parts.push(format!("{:.0}%", item.converted * 100.0));
            if let Some(encoder) = item.encoder {
                parts.push(encoder.to_owned());
            }
            (parts.join("  ·  "), MUTED)
        }
        DownloadStatus::Downloading => {
            let mut parts = vec![size];
            if let Some(fraction) = item.fraction() {
                parts.push(format!("{:.0}%", fraction * 100.0));
            }
            if item.speed > 0.0 {
                parts.push(format_speed(item.speed));
            }
            (parts.join("  ·  "), MUTED)
        }
        DownloadStatus::Done => (
            format!("Saved  ·  {}", format_bytes(item.total.unwrap_or(item.received))),
            GOOD,
        ),
        DownloadStatus::Failed(reason) => (format!("{reason}  ·  {size} saved"), DANGER),
        DownloadStatus::Cancelled => (format!("Cancelled  ·  {size} saved"), FAINT),
    }
}

fn row_actions(app: &App, item: &Download) -> Vec<Widget> {
    let id = item.id;
    let action = |glyph: std::sync::Arc<Path>, tone: Tone, call: fn(&App, u64)| {
        let app = app.clone();
        small_icon_button(glyph, None, tone, move || call(&app, id))
    };
    let mut actions = Vec::new();
    match item.status {
        DownloadStatus::Preparing | DownloadStatus::Downloading | DownloadStatus::Converting => {
            actions.push(action(icons::x(), Tone::Neutral, |app, id| app.cancel_download(id)));
        }
        DownloadStatus::Done => {
            let app = app.clone();
            actions.push(small_icon_button(icons::play(), Some("Play"), Tone::Primary, move || {
                app.play_download(id)
            }));
            actions.push(hgap(6.0));
            actions.push(action(icons::folder_open(), Tone::Neutral, |app, id| {
                app.reveal_download(id)
            }));
            actions.push(hgap(6.0));
            actions.push(action(icons::trash_2(), Tone::Danger, |app, id| app.delete_download(id)));
        }
        DownloadStatus::Failed(_) | DownloadStatus::Cancelled => {
            let app = app.clone();
            actions.push(small_icon_button(
                icons::rotate_ccw(),
                Some("Resume"),
                Tone::Neutral,
                move || app.retry_download(id),
            ));
            actions.push(hgap(6.0));
            actions.push(action(icons::trash_2(), Tone::Danger, |app, id| app.delete_download(id)));
        }
    }
    actions
}

fn download_row(app: &App, item: &Download) -> Widget {
    let (status, status_color) = status_line(item);
    let mut info: Vec<Widget> = vec![
        line(item.title.clone(), 14.5, FontWeight::W600, TEXT),
        gap(3.0),
        line(item.source.clone(), 12.0, FontWeight::W400, FAINT),
        gap(10.0),
    ];
    if item.status.is_active() {
        info.push(progress_bar(item.fraction(), ACCENT));
        info.push(gap(8.0));
    }
    info.push(line(status, 12.0, FontWeight::W500, status_color));

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
    let trailing = vec![
        text(folder, 12.0, FontWeight::W400, FAINT).into(),
        hgap(12.0),
        small_icon_button(icons::folder_open(), Some("Open folder"), Tone::Neutral, move || {
            open.open_download_folder()
        }),
    ];
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
