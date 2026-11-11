//! Startup screen: shows what is being loaded and how far along it is.

use crate::state::{App, Load};
use crate::theme::*;
use incular::prelude::*;

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Done,
    Working,
    Problem,
}

fn step_row(state: Step, label: &str, detail: String) -> Widget {
    let (color, mark): (Color, Widget) = match state {
        Step::Done => (GOOD, icon_with(icons::check(), 13.0, BG, 3.0, false)),
        Step::Working => (ACCENT, SizedBox::shrink().into()),
        Step::Problem => (DANGER, icon_with(icons::x(), 13.0, BG, 3.0, false)),
    };
    let bullet = Container::new()
        .width(18.0)
        .height(18.0)
        .radius(9.0)
        .alignment(Alignment::CENTER)
        .color(if state == Step::Working { SURFACE_HOVER } else { color })
        .child(if state == Step::Working {
            Container::new().width(6.0).height(6.0).radius(3.0).color(color).into()
        } else {
            mark
        });
    Padding::new(
        EdgeInsets::only(0.0, 0.0, 0.0, 12.0),
        hrow(vec![
            bullet.into(),
            hgap(12.0),
            text(label, 13.5, FontWeight::W600, TEXT).into(),
            Spacer::new().into(),
            text(detail, 12.5, FontWeight::W400, MUTED).into(),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .into()
}

pub fn screen(app: &App) -> Widget {
    const WIDTH: f32 = 420.0;
    let (addons, signed_in) = app.profile.read(|p| (p.addons.len(), p.auth.is_some()));
    let (rows_done, rows_total, items) = app.board.read(|b| {
        (
            b.rows.iter().filter(|r| r.items != Load::Loading).count(),
            b.rows.len(),
            b.rows
                .iter()
                .map(|r| match &r.items {
                    Load::Ready(items) => items.len(),
                    _ => 0,
                })
                .sum::<usize>(),
        )
    });
    let server = app.server_online.get();

    let addons_step = if addons > 0 { Step::Done } else { Step::Working };
    let catalogs_step = if rows_total > 0 && rows_done == rows_total {
        Step::Done
    } else {
        Step::Working
    };
    let server_step = match server {
        Some(true) => Step::Done,
        Some(false) => Step::Problem,
        None => Step::Working,
    };

    // Profile is instant; the rest fill the bar as they arrive.
    let catalog_share = if rows_total > 0 { rows_done as f32 / rows_total as f32 } else { 0.0 };
    let fraction = (0.15
        + if addons > 0 { 0.2 } else { 0.0 }
        + 0.5 * catalog_share
        + if server.is_some() { 0.15 } else { 0.0 })
    .clamp(0.0, 1.0);
    let headline = if addons == 0 {
        "Fetching your addons…"
    } else if rows_done < rows_total {
        "Loading catalogs…"
    } else {
        "Almost there…"
    };

    let skip = app.clone();
    let card = Container::new()
        .width(WIDTH + 56.0)
        .padding(EdgeInsets::all(28.0))
        .decoration(
            BoxDecoration::new()
                .color(PANEL)
                .border_radius(BorderRadius::all(Radius::circular(22.0))),
        )
        .child(
            Column::new(vec![
                row(vec![
                    logo(38.0),
                    hgap(14.0),
                    col(vec![
                        text("Spoutpeak", 22.0, FontWeight::W700, TEXT).into(),
                        gap(2.0),
                        text(headline, 13.0, FontWeight::W400, MUTED).into(),
                    ])
                    .into(),
                ])
                .into(),
                gap(22.0),
                layers(vec![
                    Container::new().width(WIDTH).height(6.0).radius(3.0).color(SURFACE_HI).into(),
                    Container::new()
                        .width((WIDTH * fraction).max(6.0))
                        .height(6.0)
                        .radius(3.0)
                        .color(ACCENT)
                        .into(),
                ])
                .into(),
                gap(8.0),
                hrow(vec![
                    text(format!("{:.0}%", fraction * 100.0), 12.0, FontWeight::W600, ACCENT).into(),
                    Spacer::new().into(),
                    text(
                        if items > 0 { format!("{items} titles ready") } else { String::new() },
                        12.0,
                        FontWeight::W400,
                        FAINT,
                    )
                    .into(),
                ])
                .into(),
                gap(22.0),
                step_row(
                    Step::Done,
                    "Profile",
                    if signed_in { "Signed in".to_owned() } else { "Guest".to_owned() },
                ),
                step_row(
                    addons_step,
                    "Addons",
                    if addons > 0 { format!("{addons} installed") } else { "Contacting…".to_owned() },
                ),
                step_row(
                    catalogs_step,
                    "Catalogs",
                    if rows_total > 0 {
                        format!("{rows_done} of {rows_total}")
                    } else {
                        "Waiting for addons".to_owned()
                    },
                ),
                step_row(
                    server_step,
                    "Streaming server",
                    match server {
                        Some(true) => "Online".to_owned(),
                        Some(false) => "Offline (torrents unavailable)".to_owned(),
                        None => "Starting…".to_owned(),
                    },
                ),
                gap(6.0),
                Align::new(
                    Alignment::CENTER_RIGHT,
                    small_button("Skip", Tone::Ghost, move || skip.finish_boot()),
                )
                .into(),
            ] as Vec<Widget>)
            .main_axis_size(MainAxisSize::Min)
            .cross_axis_alignment(CrossAxisAlignment::Stretch),
        );

    Container::new()
        .color(BG)
        .alignment(Alignment::CENTER)
        .child(card)
        .into()
}
