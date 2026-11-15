//! Scripted UI automation for development and QA, driven entirely by
//! Incular's in-process simulation (no OS input, no desktop capture).
//!
//! `SPOUTPEAK_SCRIPT="wait:4;shot:board;click:Discover;wait:3;shot:discover;exit"`
//! Steps: `wait:<seconds>`, `shot:<name>`, `click:<label>`,
//! `clickat:<x>,<y>`, `move:<x>,<y>` (pointer, for `scroll`), `type:<text>`,
//! `key:<Code>`, `scroll:<dy>` (at the pointer), `exit`.
//! Screenshots go to `SPOUTPEAK_SHOT_DIR` (default `target/shots`).

use incular::prelude::{Code, Offset};
use incular::testing::Simulation;
use std::path::PathBuf;
use std::time::Duration;

pub fn start(simulation: Simulation) {
    let Ok(script) = std::env::var("SPOUTPEAK_SCRIPT") else {
        return;
    };
    let dir = std::env::var_os("SPOUTPEAK_SHOT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/shots"));
    let _ = std::thread::Builder::new()
        .name("spoutpeak-script".into())
        .spawn(move || run(&simulation, &script, &dir));
}

fn run(simulation: &Simulation, script: &str, dir: &PathBuf) {
    let _ = std::fs::create_dir_all(dir);
    let _ = simulation.wait_for_frame();
    for step in script.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let (command, argument) = step.split_once(':').unwrap_or((step, ""));
        let outcome = match command {
            "wait" => {
                let seconds: f64 = argument.parse().unwrap_or(1.0);
                std::thread::sleep(Duration::from_secs_f64(seconds));
                Ok(())
            }
            "shot" => simulation.capture().map(|shot| {
                let path = dir.join(format!("{argument}.png"));
                let saved = image::save_buffer(
                    &path,
                    shot.pixels(),
                    shot.width(),
                    shot.height(),
                    image::ColorType::Rgba8,
                );
                eprintln!("[script] shot {} -> {saved:?}", path.display());
            }),
            "click" => simulation.click(argument),
            "doubleclickat" => {
                let (x, y) = argument.split_once(',').unwrap_or(("0", "0"));
                let position = Offset::new(x.parse().unwrap_or(0.0), y.parse().unwrap_or(0.0));
                simulation.click_at(position).and_then(|_| {
                    // Respect the recognizer's minimum interval between taps.
                    std::thread::sleep(Duration::from_millis(80));
                    simulation.click_at(position)
                })
            }
            "window" => {
                let action = argument.to_owned();
                crate::state::with_window(move |app, window| {
                    if let Some(window) = window {
                        match action.as_str() {
                            "restore" => { let _ = window.set_minimized(false); }
                            _ => {
                                eprintln!("[script] window {:?}", window.observed_state());
                                eprintln!("[script] player fullscreen={}", app.player.fullscreen.get_untracked());
                                let preview = match app.player.preview.frame.get_untracked() {
                                    crate::player::preview::PreviewFrame::Idle => "idle",
                                    crate::player::preview::PreviewFrame::Loading => "loading",
                                    crate::player::preview::PreviewFrame::Ready(_) => "ready",
                                    crate::player::preview::PreviewFrame::Unavailable => "unavailable",
                                };
                                eprintln!("[script] seek preview={preview}");
                                app.board.peek(|b| eprintln!("[script] carousel index={} paused={}",
                                    b.carousel.index.get_untracked(), b.carousel.paused.get_untracked()));
                            }
                        }
                    }
                });
                Ok(())
            }
            "resize" => {
                let (width, height) = argument.split_once(',').unwrap_or(("1320", "820"));
                let size = incular::prelude::Size::new(width.parse().unwrap_or(1320.0), height.parse().unwrap_or(820.0));
                crate::state::with_window(move |_, window| {
                    if let Some(window) = window { let _ = window.request_logical_size(size); }
                });
                Ok(())
            }
            "clickat" => {
                let (x, y) = argument.split_once(',').unwrap_or(("0", "0"));
                simulation.click_at(Offset::new(
                    x.trim().parse().unwrap_or(0.0),
                    y.trim().parse().unwrap_or(0.0),
                ))
            }
            "move" => {
                let (x, y) = argument.split_once(',').unwrap_or(("0", "0"));
                simulation.move_mouse_to(Offset::new(
                    x.trim().parse().unwrap_or(0.0),
                    y.trim().parse().unwrap_or(0.0),
                ))
            }
            "type" => simulation.type_text(argument),
            "key" => {
                let code = match argument {
                    "Enter" => Code::Enter,
                    "Space" => Code::Space,
                    "Escape" => Code::Escape,
                    "ArrowRight" => Code::ArrowRight,
                    "ArrowLeft" => Code::ArrowLeft,
                    "KeyF" => Code::KeyF,
                    "F11" => Code::F11,
                    "KeyM" => Code::KeyM,
                    _ => Code::Tab,
                };
                simulation.press(code)
            }
            "scroll" => simulation.scroll(Offset::new(0.0, argument.parse().unwrap_or(0.0))),
            "exit" => std::process::exit(0),
            other => {
                eprintln!("[script] unknown step {other:?}");
                Ok(())
            }
        };
        match outcome {
            Ok(()) => eprintln!("[script] {step}"),
            Err(error @ incular::testing::SimulationError::AppPanicked(_)) => {
                eprintln!("[script] {step} FAILED: {error}");
                std::process::exit(101);
            }
            Err(error) => eprintln!("[script] {step} FAILED: {error}"),
        }
        let _ = simulation.wait_for_frame();
    }
}
