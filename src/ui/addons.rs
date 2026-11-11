//! Addon management: installed list, community catalog, install by URL.

use super::widgets::*;
use crate::state::{AddonsTab, App, Load};
use crate::addons::configure_url;
use crate::addons::types::Descriptor;
use crate::theme::*;
use incular::prelude::*;
use std::rc::Rc;

const CARD_HEIGHT: f32 = 112.0;

fn addon_card(app: &App, addon: &Descriptor, installed: bool, manage: bool) -> Widget {
    let manifest = &addon.manifest;
    let mut tags: Vec<String> = manifest.types.iter().take(5).cloned().collect();
    if manifest.behavior_hints.adult {
        tags.push("adult".into());
    }
    if manifest.behavior_hints.p2p {
        tags.push("p2p".into());
    }
    if addon.flags.official {
        tags.push("official".into());
    }
    let info = col(vec![
        row(vec![
            text(manifest.name.clone(), 15.0, FontWeight::W600, TEXT).into(),
            hgap(8.0),
            text(format!("v{}", manifest.version), 11.5, FontWeight::W400, FAINT).into(),
        ])
        .into(),
        gap(4.0),
        Text::new(manifest.description.clone().unwrap_or_default())
            .style(style(12.5, FontWeight::W400, MUTED))
            .max_lines(Some(2))
            .overflow(TextOverflow::Ellipsis)
            .into(),
        gap(6.0),
        line(tags.join("  ·  "), 11.5, FontWeight::W500, FAINT),
    ]);

    let mut actions: Vec<Widget> = Vec::new();
    let url = addon.transport_url.clone();
    if manage {
        for (glyph, delta) in [(icons::chevron_up(), -1), (icons::chevron_down(), 1)] {
            let (app, url) = (app.clone(), url.clone());
            actions.push(small_icon_button(glyph, None, Tone::Ghost, move || {
                app.move_addon(&url, delta)
            }));
        }
        actions.push(hgap(6.0));
    }
    if manifest.behavior_hints.configurable {
        let (app, target) = (app.clone(), configure_url(&url));
        actions.push(small_button("Configure", Tone::Neutral, move || {
            if open::that_detached(&target).is_err() {
                app.notify("Could not open the browser");
            }
        }));
        actions.push(hgap(8.0));
    }
    if installed {
        if addon.flags.protected {
            actions.push(text("Built in", 12.0, FontWeight::W500, FAINT).into());
        } else if manage {
            let (app, url) = (app.clone(), url.clone());
            actions.push(small_button("Uninstall", Tone::Danger, move || {
                app.uninstall_addon(&url)
            }));
        } else {
            actions.push(text("Installed", 12.0, FontWeight::W600, GOOD).into());
        }
    } else {
        let (app, addon) = (app.clone(), addon.clone());
        actions.push(small_button("Install", Tone::Primary, move || {
            app.add_addon(addon.clone())
        }));
    }

    Padding::new(
        EdgeInsets::only(0.0, 0.0, 0.0, 12.0),
        Container::new()
            .height(CARD_HEIGHT - 12.0)
            .padding(EdgeInsets::symmetric(16.0, 0.0))
            .alignment(Alignment::CENTER_LEFT)
            .decoration(
                BoxDecoration::new()
                    .color(SURFACE)
                    .border_radius(BorderRadius::all(Radius::circular(14.0))),
            )
            .child(
                hrow(vec![
                    logo_image(manifest.logo.as_deref(), 60.0, &manifest.name),
                    hgap(16.0),
                    Expanded::new(info).into(),
                    hgap(16.0),
                    row(actions).into(),
                ])
                .cross_axis_alignment(CrossAxisAlignment::Center),
            ),
    )
    .into()
}

pub fn screen(app: &App) -> Widget {
    let (tab, community, filter, busy) = app
        .addons
        .read(|a| (a.tab, a.community.clone(), a.filter.clone(), a.busy));
    let installed: Rc<Vec<Descriptor>> = Rc::new(app.profile.read(|p| p.addons.clone()));

    let tabs = [
        (format!("Installed ({})", installed.len()), AddonsTab::Installed),
        ("Community".to_owned(), AddonsTab::Community),
    ]
    .into_iter()
    .flat_map(|(label, value)| {
        let app = app.clone();
        [
            chip(label, tab == value, move || {
                app.addons.write(|a| a.tab = value);
                if value == AddonsTab::Community {
                    app.ensure_community();
                }
            }),
            hgap(8.0),
        ]
    })
    .collect::<Vec<_>>();

    let install = {
        let submit = app.clone();
        let click = app.clone();
        row(vec![
            input(
                &app.controllers.addon_url,
                "Paste an addon URL (https://…/manifest.json or an install link)",
                460.0,
                move |_| submit.install_addon_from_input(),
            )
            .into(),
            hgap(10.0),
            button(
                if busy { "Installing…" } else { "Install" },
                Tone::Primary,
                move || click.install_addon_from_input(),
            ),
        ])
    };

    let mut toolbar: Vec<Widget> = tabs;
    toolbar.push(Spacer::new().into());
    if tab == AddonsTab::Community {
        let app_filter = app.clone();
        toolbar.push(
            input(&app.controllers.addon_filter, "Filter addons", 220.0, |_| {})
                .on_changed(move |value| app_filter.addons.write(|a| a.filter = value))
                .into(),
        );
    }

    let list: Widget = match tab {
        AddonsTab::Installed => {
            let controller = app.controllers.addons.clone();
            let app = app.clone();
            let items = installed.clone();
            ListView::builder(items.len(), move |index| {
                addon_card(&app, &items[index], true, true)
            })
            .item_extent(CARD_HEIGHT)
            .padding(PAGE_PADDING)
            .controller(controller)
            .into()
        }
        AddonsTab::Community => match community {
            Load::Ready(all) => {
                let needle = filter.to_lowercase();
                let filtered: Rc<Vec<Descriptor>> = Rc::new(
                    all.into_iter()
                        .filter(|a| {
                            needle.is_empty()
                                || a.manifest.name.to_lowercase().contains(&needle)
                                || a.manifest
                                    .description
                                    .as_deref()
                                    .unwrap_or("")
                                    .to_lowercase()
                                    .contains(&needle)
                        })
                        .collect(),
                );
                let app = app.clone();
                let installed = installed.clone();
                ListView::builder(filtered.len(), move |index| {
                    let addon = &filtered[index];
                    let is_installed = installed.iter().any(|i| {
                        i.transport_url == addon.transport_url || i.manifest.id == addon.manifest.id
                    });
                    addon_card(&app, addon, is_installed, false)
                })
                .item_extent(CARD_HEIGHT)
                .padding(PAGE_PADDING)
                .into()
            }
            Load::Failed(error) => Padding::new(
                PAGE_PADDING,
                notice(format!("Couldn't load community addons: {error}"), DANGER),
            )
            .into(),
            _ => Padding::new(PAGE_PADDING, notice("Loading community addons…", MUTED)).into(),
        },
    };

    page(
        "Addons",
        vec![install.into()],
        vcol(vec![
            Padding::new(
                EdgeInsets::only(32.0, 0.0, 32.0, 14.0),
                Row::new(toolbar).cross_axis_alignment(CrossAxisAlignment::Center),
            )
            .into(),
            Expanded::new(list).into(),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Stretch)
        .into(),
    )
}
