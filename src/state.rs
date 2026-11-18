//! Application state and the actions that mutate it.
//!
//! All state lives on the UI thread. Background work (network, decoding)
//! runs on Tokio or plain threads and comes back through [`ui`], which
//! re-enters the UI thread and hands the callback the [`App`].

use crate::player::PlayerUi;
use crate::store::{self, Profile};
use crate::addons::{self, CatalogRef};
use crate::addons::types::*;
use incular::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

// ------------------------------------------------------------------ bridge

struct Bridge {
    tokio: TokioHandle,
    ui: Mutex<UiDispatcher>,
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Registers the application state for the current (UI) thread.
pub fn install_app(app: App) {
    APP.with(|slot| *slot.borrow_mut() = Some(app));
}

/// Connects background work to the UI thread. Must run on the UI thread.
pub fn install_bridge(tokio: TokioHandle, dispatcher: UiDispatcher) {
    let _ = BRIDGE.set(Bridge {
        tokio,
        ui: Mutex::new(dispatcher),
    });
}

/// The application state, when called on the UI thread after startup.
pub fn with_app(call: impl FnOnce(&App)) {
    let app = APP.with(|slot| slot.borrow().clone());
    if let Some(app) = app {
        call(&app);
    }
}

/// Copy through the active native runtime and report errors rather than
/// claiming success if the desktop clipboard is temporarily unavailable.
pub fn copy_text(text: String, success: String) {
    let Some(bridge) = BRIDGE.get() else { return };
    let dispatcher = bridge.ui.lock().expect("ui dispatcher").clone();
    dispatcher.dispatch(move |runtime| {
        let result = runtime.write_clipboard(DataTransfer::plain_text(text));
        with_app(|app| match result {
            Ok(report) if !report.written.is_empty() => app.notify(success),
            Ok(_) => app.notify("Clipboard is unavailable"),
            Err(error) => app.notify(format!("Could not copy: {error}")),
        });
    });
}

/// Runs `callback` on the UI thread. Callable from any thread.
pub fn ui(callback: impl FnOnce(&App) + Send + 'static) {
    let Some(bridge) = BRIDGE.get() else { return };
    let dispatcher = bridge.ui.lock().expect("ui dispatcher").clone();
    dispatcher.dispatch(move |_runtime| {
        let app = APP.with(|slot| slot.borrow().clone());
        if let Some(app) = app {
            callback(&app);
        }
    });
}

/// Hands `callback` the window handle, which only the running runtime knows.
pub fn with_window(callback: impl FnOnce(&App, Option<WindowHandle>) + Send + 'static) {
    let Some(bridge) = BRIDGE.get() else { return };
    let dispatcher = bridge.ui.lock().expect("ui dispatcher").clone();
    dispatcher.dispatch(move |runtime| {
        let window = runtime.window_handle();
        let app = APP.with(|slot| slot.borrow().clone());
        if let Some(app) = app {
            // Application-dispatched callbacks have no window scope; retain
            // access to the main window captured when the app was built.
            let window = window.or_else(|| app.player.window.borrow().clone());
            callback(&app, window);
        }
    });
}

/// Runs `future` on Tokio and delivers its output on the UI thread.
pub fn spawn<T: Send + 'static>(
    future: impl Future<Output = T> + Send + 'static,
    done: impl FnOnce(&App, T) + Send + 'static,
) {
    let Some(bridge) = BRIDGE.get() else { return };
    bridge.tokio.spawn(async move {
        let output = future.await;
        ui(move |app| done(app, output));
    });
}

pub fn tokio() -> Option<TokioHandle> {
    BRIDGE.get().map(|b| b.tokio.clone())
}

// ------------------------------------------------------------------ shared

/// Interior-mutable state with a revision signal: reading inside a builder
/// subscribes it, writing schedules a rebuild. Avoids cloning large values
/// through `Signal<T>`.
pub struct Shared<T> {
    data: Rc<RefCell<T>>,
    revision: Signal<u64>,
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            revision: self.revision.clone(),
        }
    }
}

impl<T: 'static> Shared<T> {
    pub fn new(value: T) -> Self {
        Self {
            data: Rc::new(RefCell::new(value)),
            revision: Signal::new(0),
        }
    }
    /// Tracked read.
    pub fn read<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        let _ = self.revision.get();
        read(&self.data.borrow())
    }
    /// Untracked read, for callbacks.
    pub fn peek<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        read(&self.data.borrow())
    }
    pub fn write<R>(&self, write: impl FnOnce(&mut T) -> R) -> R {
        let result = write(&mut self.data.borrow_mut());
        self.revision.update(|r| *r += 1);
        result
    }
    /// Mutates without notifying; for bookkeeping that never affects UI.
    pub fn write_silent<R>(&self, write: impl FnOnce(&mut T) -> R) -> R {
        write(&mut self.data.borrow_mut())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Load<T> {
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

// ------------------------------------------------------------------- model

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Board,
    Discover,
    Library,
    Downloads,
    Search,
    Addons,
    Settings,
    Together,
    Detail,
    Player,
}

#[derive(Clone, Debug)]
pub struct CatalogRow {
    pub key: String,
    pub title: String,
    pub addon_name: String,
    pub items: Load<Vec<Meta>>,
    /// Shown as a row on the Board (a Spotlight catalog may be fetched only
    /// for the Spotlight).
    pub on_board: bool,
}

#[derive(Default)]
pub struct BoardState {
    pub rows: Vec<CatalogRow>,
    /// Catalog keys the Spotlight takes turns between, in order.
    pub spotlight: Vec<String>,
    pub featured: Vec<(Meta, String)>,
    pub generation: u64,
    pub carousel: FeaturedCarousel,
}

/// Separate signals keep slide changes from rebuilding the catalog rows.
#[derive(Clone)]
pub struct FeaturedCarousel {
    pub index: Signal<usize>,
    pub paused: Signal<bool>,
    pub progress: Signal<f32>,
    pub hovered: Rc<Cell<bool>>,
    elapsed: Rc<Cell<std::time::Duration>>,
    updated: Rc<Cell<std::time::Instant>>,
}

const FEATURED_INTERVAL: std::time::Duration = std::time::Duration::from_secs(8);

impl Default for FeaturedCarousel {
    fn default() -> Self {
        Self {
            index: Signal::new(0),
            paused: Signal::new(false),
            progress: Signal::new(0.0),
            hovered: Rc::new(Cell::new(false)),
            elapsed: Rc::new(Cell::new(std::time::Duration::ZERO)),
            updated: Rc::new(Cell::new(std::time::Instant::now())),
        }
    }
}

impl FeaturedCarousel {
    pub fn select(&self, index: usize) {
        self.reset_timer();
        self.index.set(index);
    }

    pub fn advance(&self, direction: isize, count: usize) {
        if count > 1 {
            self.select((self.index.get_untracked() as isize + direction).rem_euclid(count as isize) as usize);
        }
    }

    pub fn hover(&self, hovered: bool) {
        self.hovered.set(hovered);
        self.updated.set(std::time::Instant::now());
    }

    pub fn toggle_pause(&self) {
        self.updated.set(std::time::Instant::now());
        self.paused.set(!self.paused.get_untracked());
    }

    fn reset_timer(&self) {
        self.updated.set(std::time::Instant::now());
        self.elapsed.set(std::time::Duration::ZERO);
        self.progress.set(0.0);
    }

    /// One clock drives both the indicator and the automatic slide change.
    fn tick(&self, now: std::time::Instant, on_screen: bool) -> bool {
        let delta = now.saturating_duration_since(self.updated.replace(now));
        if !on_screen {
            self.elapsed.set(std::time::Duration::ZERO);
            self.progress.set(0.0);
        } else if !self.hovered.get() && !self.paused.get_untracked()
            && !incular::animation::disable_animations()
        {
            let elapsed = (self.elapsed.get() + delta).min(FEATURED_INTERVAL);
            self.elapsed.set(elapsed);
            self.progress.set(elapsed.as_secs_f32() / FEATURED_INTERVAL.as_secs_f32());
            return elapsed >= FEATURED_INTERVAL;
        }
        false
    }
}

#[cfg(test)]
mod carousel_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn progress_reaches_deadline_and_resets_on_selection_and_wrap() {
        let carousel = FeaturedCarousel::default();
        let start = carousel.updated.get();
        assert!(!carousel.tick(start + Duration::from_secs(4), true));
        assert_eq!(carousel.progress.get_untracked(), 0.5);
        assert!(carousel.tick(start + Duration::from_secs(8), true));
        assert_eq!(carousel.progress.get_untracked(), 1.0);
        carousel.advance(1, 3);
        assert_eq!(carousel.index.get_untracked(), 1);
        assert_eq!(carousel.progress.get_untracked(), 0.0);
        carousel.select(2);
        carousel.advance(1, 3);
        assert_eq!(carousel.index.get_untracked(), 0);
        assert_eq!(carousel.elapsed.get(), Duration::ZERO);
    }

    #[test]
    fn pause_and_hover_hold_progress_and_resume_remaining_time() {
        let carousel = FeaturedCarousel::default();
        let start = carousel.updated.get();
        carousel.tick(start + Duration::from_secs(2), true);
        carousel.toggle_pause();
        let paused = carousel.updated.get();
        assert!(!carousel.tick(paused + Duration::from_secs(20), true));
        assert_eq!(carousel.progress.get_untracked(), 0.25);
        carousel.toggle_pause();
        let resumed = carousel.updated.get();
        carousel.tick(resumed + Duration::from_secs(2), true);
        carousel.hover(true);
        let hovered = carousel.updated.get();
        assert!(!carousel.tick(hovered + Duration::from_secs(20), true));
        assert_eq!(carousel.progress.get_untracked(), 0.5);
        carousel.hover(false);
        let resumed = carousel.updated.get();
        assert!(carousel.tick(resumed + Duration::from_secs(4), true));
    }

    #[test]
    fn leaving_carousel_restarts_the_visible_timer() {
        let carousel = FeaturedCarousel::default();
        let start = carousel.updated.get();
        carousel.tick(start + Duration::from_secs(7), true);
        assert!(!carousel.tick(start + Duration::from_secs(8), false));
        assert_eq!(carousel.progress.get_untracked(), 0.0);
        assert!(!carousel.tick(start + Duration::from_secs(9), true));
        assert_eq!(carousel.progress.get_untracked(), 0.125);
    }
}

/// Titles per Spotlight catalog.
pub const SPOTLIGHT_TITLES: usize = 6;
/// Catalogs the Spotlight can take turns between.
pub const SPOTLIGHT_MAX: usize = 3;

/// A catalog's titles the Spotlight can show: with an id and artwork.
pub fn spotlight_candidates(items: &[Meta]) -> impl Iterator<Item = &Meta> {
    items.iter()
        .filter(|meta| [meta.background.as_ref(), meta.poster.as_ref()].into_iter()
            .flatten().any(|url| !url.trim().is_empty()))
        .filter(|meta| !meta.id().trim().is_empty())
}

/// The Spotlight slides: up to [`SPOTLIGHT_TITLES`] titles with artwork from
/// each catalog in `keys` that has loaded, taking turns in `keys` order. A
/// title already taken by an earlier catalog is skipped.
fn featured_titles(rows: &[CatalogRow], keys: &[String]) -> Vec<(Meta, String)> {
    let mut seen = HashSet::new();
    let picks: Vec<Vec<(Meta, String)>> = keys.iter()
        .filter_map(|key| rows.iter().find(|row| &row.key == key))
        .map(|row| match &row.items {
            Load::Ready(items) => spotlight_candidates(items)
                .filter(|meta| seen.insert((meta.kind.clone(), meta.id().to_owned())))
                .take(SPOTLIGHT_TITLES)
                .map(|meta| (meta.clone(), row.title.clone()))
                .collect(),
            _ => Vec::new(),
        })
        .collect();
    let longest = picks.iter().map(Vec::len).max().unwrap_or(0);
    (0..longest)
        .flat_map(|turn| picks.iter().filter_map(move |pick| pick.get(turn).cloned()))
        .collect()
}

#[cfg(test)]
mod featured_catalog_tests {
    use super::*;

    fn row(key: &str, ids: &[&str]) -> CatalogRow {
        CatalogRow {
            key: key.into(),
            title: format!("{key} catalog"),
            addon_name: key.into(),
            items: Load::Ready(ids.iter().map(|id| Meta {
                id: Some((*id).into()), kind: "movie".into(),
                background: Some(format!("https://example.test/{id}.jpg")),
                ..Meta::default()
            }).collect()),
            on_board: true,
        }
    }

    fn ids(featured: &[(Meta, String)]) -> Vec<&str> {
        featured.iter().map(|(m, _)| m.id()).collect()
    }

    fn keys(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|k| (*k).to_owned()).collect()
    }

    #[test]
    fn chosen_catalogs_take_turns_in_their_order() {
        let rows = [row("alpha", &["a1", "a2", "a3"]), row("beta", &["b1"]), row("gamma", &["g1", "g2"])];
        assert_eq!(ids(&featured_titles(&rows, &keys(&["gamma", "alpha", "beta"]))),
            ["g1", "a1", "b1", "g2", "a2", "a3"]);
        assert_eq!(ids(&featured_titles(&rows, &keys(&["beta"]))), ["b1"]);
        // Unknown and unloaded catalogs add nothing.
        let mut loading = row("beta", &[]);
        loading.items = Load::Loading;
        assert!(featured_titles(&[loading], &keys(&["beta", "missing"])).is_empty());
    }

    #[test]
    fn each_catalog_gives_at_most_six_titles() {
        let many: Vec<String> = (0..10).map(|i| format!("m{i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        let featured = featured_titles(&[row("many", &many)], &keys(&["many"]));
        assert_eq!(featured.len(), SPOTLIGHT_TITLES);
        assert!(featured.iter().all(|(_, title)| title == "many catalog"));
    }

    #[test]
    fn duplicates_do_not_use_another_catalogs_selection() {
        let rows = [row("first", &["same", "a"]), row("other", &["same", "b"])];
        assert_eq!(ids(&featured_titles(&rows, &keys(&["first", "other"]))), ["same", "b", "a"]);
        // A different media type with the same ID remains a separate title.
        let mut series = row("series", &["same"]);
        if let Load::Ready(items) = &mut series.items { items[0].kind = "series".into(); }
        assert_eq!(featured_titles(&[rows[0].clone(), series], &keys(&["first", "series"])).len(), 3);
    }

    #[test]
    fn only_valid_titles_with_artwork_are_featured() {
        let mut catalog = row("any-provider", &["", "no-art", "blank-art", "poster", "background"]);
        if let Load::Ready(items) = &mut catalog.items {
            items[1].background = None;
            items[2].background = Some("  ".into());
            items[3].background = None;
            items[3].poster = Some("https://example.test/poster.jpg".into());
        }
        assert_eq!(ids(&featured_titles(&[catalog], &keys(&["any-provider"]))), ["poster", "background"]);
    }
}

#[derive(Default)]
pub struct DiscoverState {
    pub kind: String,
    pub catalog_key: String,
    pub genre: Option<String>,
    pub items: Vec<Meta>,
    pub loading: bool,
    pub exhausted: bool,
    pub error: Option<String>,
    pub generation: u64,
}

#[derive(Default)]
pub struct SearchState {
    pub query: String,
    pub rows: Vec<CatalogRow>,
    pub generation: u64,
}

#[derive(Clone, Debug)]
pub struct StreamGroup {
    pub addon_name: String,
    pub items: Load<Vec<Stream>>,
}

/// Stream list quality filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    All,
    Uhd,
    Fhd,
    Hd,
    Sd,
}

impl Quality {
    pub const ALL: [Quality; 5] = [Self::All, Self::Uhd, Self::Fhd, Self::Hd, Self::Sd];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Uhd => "4K",
            Self::Fhd => "1080p",
            Self::Hd => "720p",
            Self::Sd => "SD",
        }
    }

    pub fn of(stream: &Stream) -> Quality {
        let text = format!("{} {}", stream.headline(), stream.details()).to_lowercase();
        if text.contains("2160") || text.contains("4k") || text.contains("uhd") {
            Self::Uhd
        } else if text.contains("1080") {
            Self::Fhd
        } else if text.contains("720") {
            Self::Hd
        } else {
            Self::Sd
        }
    }
}

pub struct DetailState {
    pub quality: Quality,
    pub kind: String,
    pub id: String,
    pub preview: Option<Meta>,
    pub meta: Load<Meta>,
    pub season: Option<u32>,
    pub video: Option<Video>,
    pub streams: Vec<StreamGroup>,
    pub generation: u64,
}

impl Default for DetailState {
    fn default() -> Self {
        Self {
            quality: Quality::All,
            kind: String::new(),
            id: String::new(),
            preview: None,
            meta: Load::Idle,
            season: None,
            video: None,
            streams: Vec::new(),
            generation: 0,
        }
    }
}

impl DetailState {
    /// Best available metadata: the full meta, else the catalog preview.
    pub fn best_meta(&self) -> Option<Meta> {
        match &self.meta {
            Load::Ready(meta) => Some(meta.clone()),
            _ => self.preview.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddonsTab {
    Installed,
    Community,
}

pub struct AddonsState {
    pub tab: AddonsTab,
    pub community: Load<Vec<Descriptor>>,
    pub filter: String,
    pub busy: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibrarySort {
    Recent,
    Name,
    Watched,
}

pub struct LibraryView {
    pub kind: Option<String>,
    pub sort: LibrarySort,
}

/// Text controllers and scroll positions must outlive rebuilds.
#[derive(Clone)]
pub struct Controllers {
    pub search: TextEditingController,
    pub addon_url: TextEditingController,
    pub addon_filter: TextEditingController,
    pub open_url: TextEditingController,
    /// Keyboard focus for global shortcuts; must outlive rebuilds.
    pub focus: FocusNode,
    pub player_focus: FocusNode,
    pub player_path: TextEditingController,
    pub download_dir: TextEditingController,
    pub watch_name: TextEditingController,
    pub watch_code: TextEditingController,
    pub watch_password: TextEditingController,
    pub watch_join_password: TextEditingController,
    pub together: ScrollController,
    pub watch_chat: TextEditingController,
    pub watch_chat_scroll: ScrollController,
    pub board: ScrollController,
    pub discover: ScrollController,
    pub library: ScrollController,
    pub search_results: ScrollController,
    pub detail: ScrollController,
    pub streams: ScrollController,
    pub sync_list: ScrollController,
    /// The player's audio / subtitle / speed panels.
    pub player_menu: ScrollController,
    pub season_search: TextEditingController,
    pub season_list: ScrollController,
    pub spotlight_search: TextEditingController,
    pub spotlight_list: ScrollController,
    pub downloads: ScrollController,
    pub addons: ScrollController,
    pub settings: ScrollController,
    rows: Rc<RefCell<HashMap<String, ScrollController>>>,
}

impl Controllers {
    fn new(profile: &Profile) -> Self {
        Self {
            search: TextEditingController::new(),
            addon_url: TextEditingController::new(),
            addon_filter: TextEditingController::new(),
            open_url: TextEditingController::new(),
            focus: FocusNode::new(),
            player_focus: FocusNode::new(),
            player_path: TextEditingController::with_text(&profile.settings.custom_player_path),
            download_dir: TextEditingController::with_text(&profile.settings.download_dir),
            watch_name: TextEditingController::with_text(&profile.settings.watch_name),
            watch_code: TextEditingController::new(),
            watch_password: TextEditingController::new(),
            watch_join_password: TextEditingController::new(),
            together: ScrollController::new(),
            watch_chat: TextEditingController::new(),
            watch_chat_scroll: ScrollController::new(),
            board: ScrollController::new(),
            discover: ScrollController::new(),
            library: ScrollController::new(),
            search_results: ScrollController::new(),
            detail: ScrollController::new(),
            streams: ScrollController::new(),
            sync_list: ScrollController::new(),
            player_menu: ScrollController::new(),
            season_search: TextEditingController::new(),
            season_list: ScrollController::new(),
            spotlight_search: TextEditingController::new(),
            spotlight_list: ScrollController::new(),
            downloads: ScrollController::new(),
            addons: ScrollController::new(),
            settings: ScrollController::new(),
            rows: Rc::new(RefCell::new(HashMap::new())),
        }
    }
    /// Stable horizontal scroll controller for a catalog row.
    pub fn row(&self, key: &str) -> ScrollController {
        self.rows
            .borrow_mut()
            .entry(key.to_owned())
            .or_insert_with(ScrollController::new)
            .clone()
    }
}

#[derive(Clone)]
pub struct App {
    pub route: Signal<Vec<Route>>,
    pub toast: Signal<Option<String>>,
    toast_generation: Rc<RefCell<u64>>,
    pub profile: Shared<Profile>,
    pub library: Shared<HashMap<String, LibraryItem>>,
    pub library_view: Shared<LibraryView>,
    pub board: Shared<BoardState>,
    pub discover: Shared<DiscoverState>,
    pub search: Shared<SearchState>,
    pub detail: Shared<DetailState>,
    pub together: crate::together::Together,
    /// Season combobox on the detail page. Separate signals so typing in
    /// its search field rebuilds only the result list, not the field.
    pub season_open: Signal<bool>,
    pub season_filter: Signal<String>,
    pub addons: Shared<AddonsState>,
    /// Built-in torrent engine: `None` while starting, then whether it runs.
    pub server_online: Signal<Option<bool>>,
    search_typing: Rc<std::cell::Cell<u64>>,
    /// The history and suggestions dropdown under a search field.
    pub search_menu: Signal<bool>,
    pub search_suggestions: Signal<Vec<(u8, crate::suggest::Suggestion)>>,
    /// What the dropdown matched against.
    pub search_menu_query: Signal<String>,
    search_feed: Rc<RefCell<Load<Rc<Vec<crate::suggest::Entry>>>>>,
    /// The Spotlight catalog picker (a dialog over the app), its search
    /// text, and each catalog's titles for the picker's previews.
    pub spotlight_picker: Signal<bool>,
    /// Artwork shown whole over the app (see `ui::artwork`).
    pub artwork_view: Signal<Option<String>>,
    pub spotlight_filter: Signal<String>,
    pub spotlight_preview: Shared<HashMap<String, Load<Vec<Meta>>>>,
    pub player: PlayerUi,
    pub ai: crate::ai::LocalAi,
    pub downloads: Shared<Vec<crate::downloads::Download>>,
    pub controllers: Controllers,
}

impl App {
    pub fn new() -> Self {
        let profile = store::load_profile();
        tracing::info!(player = ?profile.settings.player,
            hardware_decoding = profile.settings.hardware_decoding,
            max_video_height = profile.settings.max_video_height,
            fast_subtitles = profile.settings.fast_subtitles,
            local_ai_enabled = profile.settings.local_ai_enabled,
            automatic_subtitle_sync = profile.settings.automatic_subtitle_sync,
            peer_encryption = ?profile.settings.torrent.encryption,
            addon_count = profile.addons.len(), "Loaded playback configuration");
        Self::with_profile(profile)
    }

    pub(crate) fn with_profile(profile: Profile) -> Self {
        let controllers = Controllers::new(&profile);
        crate::downloads::set_custom_folder(&profile.settings.download_dir);
        Self {
            route: Signal::new(vec![Route::Board]),
            toast: Signal::new(None),
            toast_generation: Rc::new(RefCell::new(0)),
            player: PlayerUi::new(&profile.settings),
            ai: crate::ai::LocalAi::new(),
            profile: Shared::new(profile),
            library: Shared::new(store::load_library()),
            library_view: Shared::new(LibraryView {
                kind: None,
                sort: LibrarySort::Recent,
            }),
            board: Shared::new(BoardState::default()),
            discover: Shared::new(DiscoverState::default()),
            search: Shared::new(SearchState::default()),
            detail: Shared::new(DetailState::default()),
            together: crate::together::Together::new(),
            season_open: Signal::new(false),
            season_filter: Signal::new(String::new()),
            addons: Shared::new(AddonsState {
                tab: AddonsTab::Installed,
                community: Load::Idle,
                filter: String::new(),
                busy: false,
            }),
            server_online: Signal::new(None),
            search_typing: Rc::new(std::cell::Cell::new(0)),
            search_menu: Signal::new(false),
            search_suggestions: Signal::new(Vec::new()),
            search_menu_query: Signal::new(String::new()),
            search_feed: Rc::new(RefCell::new(Load::Idle)),
            spotlight_picker: Signal::new(false),
            artwork_view: Signal::new(None),
            spotlight_filter: Signal::new(String::new()),
            spotlight_preview: Shared::new(HashMap::new()),
            downloads: Shared::new(crate::downloads::load()),
            controllers,
        }
    }

    // ------------------------------------------------------------ startup

    pub fn startup(&self) {
        tracing::info!("Starting background services");
        let has_addons = self.profile.peek(|p| !p.addons.is_empty());
        if has_addons {
            self.load_board();
        } else {
            spawn(addons::default_addons(), |app, addons| {
                if addons.is_empty() {
                    app.notify("Could not reach the default addons. Check your connection.");
                }
                app.profile.write(|p| {
                    if p.addons.is_empty() {
                        p.addons = addons;
                    }
                });
                app.save_profile();
                app.load_board();
            });
        }
        self.start_streaming();
        self.downloads.peek(|list| crate::downloads::clean_staging(list));
        self.resume_interrupted_downloads();
        // Find the video encoder off the UI thread (it test-encodes a clip).
        std::thread::spawn(|| {
            let _ = crate::convert::encoder();
        });
    }

    pub fn save_profile(&self) {
        self.profile.peek(store::save_profile);
    }

    pub fn notify(&self, message: impl Into<String>) {
        let message = message.into();
        tracing::info!(message = %message, "User notification");
        let generation = {
            let mut generation = self.toast_generation.borrow_mut();
            *generation += 1;
            *generation
        };
        self.toast.set(Some(message));
        spawn(
            // Built inside the task: a Tokio timer needs the runtime context.
            async { tokio::time::sleep(std::time::Duration::from_millis(4200)).await },
            move |app, ()| {
                if *app.toast_generation.borrow() == generation {
                    app.toast.set(None);
                }
            },
        );
    }

    // --------------------------------------------------------- navigation

    pub fn current_route(&self) -> Route {
        self.route.get().last().copied().unwrap_or(Route::Board)
    }

    pub fn go(&self, route: Route) {
        tracing::info!(?route, "Navigating");
        if std::env::var_os("SPOUTPEAK_DEBUG").is_some() {
            eprintln!("[nav] go {route:?}");
        }
        self.leave_player_if_needed();
        self.route.set(vec![route]);
        self.controllers.focus.request_focus();
        match route {
            Route::Discover => self.ensure_discover(),
            Route::Addons => self.ensure_community(),
            _ => {}
        }
    }

    pub fn push(&self, route: Route) {
        self.route.update(|stack| {
            if stack.last() != Some(&route) {
                stack.push(route);
            }
        });
    }

    pub fn back(&self) {
        self.leave_player_if_needed();
        self.route.update(|stack| {
            if stack.len() > 1 {
                stack.pop();
            }
        });
        self.controllers.focus.request_focus();
    }

    fn leave_player_if_needed(&self) {
        if self.route.get_untracked().last() == Some(&Route::Player) {
            self.stop_playback();
        }
    }

    // -------------------------------------------------------------- board

    fn row_for(reference: &CatalogRef) -> CatalogRow {
        CatalogRow {
            key: reference.key(),
            title: reference.title(),
            addon_name: reference.addon.manifest.name.clone(),
            items: Load::Loading,
            on_board: true,
        }
    }

    /// Catalogs the Spotlight can use: every browsable one.
    pub fn spotlight_catalogs(&self) -> Vec<CatalogRef> {
        let addons = self.profile.peek(|p| p.addons.clone());
        addons::all_catalogs(&addons).into_iter().filter(|c| c.catalog.is_browsable()).collect()
    }

    /// The Spotlight's catalogs when none are chosen: the first two.
    pub fn default_spotlight_keys(catalogs: &[CatalogRef]) -> Vec<String> {
        catalogs.iter().take(2).map(CatalogRef::key).collect()
    }

    /// The chosen Spotlight catalogs that are still installed, else the
    /// defaults.
    pub fn spotlight_keys(&self) -> Vec<String> {
        let catalogs = self.spotlight_catalogs();
        let chosen: Vec<String> = self.profile.peek(|p| p.settings.spotlight_catalogs.clone())
            .into_iter()
            .filter(|key| catalogs.iter().any(|c| &c.key() == key))
            .take(SPOTLIGHT_MAX)
            .collect();
        if chosen.is_empty() { Self::default_spotlight_keys(&catalogs) } else { chosen }
    }

    pub fn set_spotlight_catalogs(&self, keys: Vec<String>) {
        self.update_settings(|s| s.spotlight_catalogs = keys);
        self.load_board();
    }

    /// Adds `key` to the Spotlight, or takes it out.
    pub fn toggle_spotlight_catalog(&self, key: String) {
        let mut keys = self.spotlight_keys();
        if let Some(index) = keys.iter().position(|k| *k == key) {
            if keys.len() == 1 {
                self.notify("The Spotlight needs a catalog. Turn the Spotlight off instead.");
                return;
            }
            keys.remove(index);
        } else if keys.len() < SPOTLIGHT_MAX {
            keys.push(key);
        } else {
            self.notify(format!("The Spotlight takes up to {SPOTLIGHT_MAX} catalogs. Remove one first."));
            return;
        }
        self.set_spotlight_catalogs(keys);
    }

    /// Opens the catalog picker and fetches every catalog the Board has
    /// not loaded, for the previews.
    pub fn open_spotlight_picker(&self) {
        self.controllers.spotlight_search.set_text("");
        self.spotlight_filter.set(String::new());
        let loaded: HashMap<String, Load<Vec<Meta>>> = self.board.peek(|b| {
            b.rows.iter().filter(|r| matches!(r.items, Load::Ready(_))).map(|r| (r.key.clone(), r.items.clone())).collect()
        });
        let catalogs = self.spotlight_catalogs();
        let missing: Vec<CatalogRef> = self.spotlight_preview.write(|preview| {
            preview.extend(loaded);
            let missing: Vec<CatalogRef> = catalogs.into_iter()
                .filter(|c| !matches!(preview.get(&c.key()), Some(Load::Ready(_) | Load::Loading)))
                .collect();
            for c in &missing {
                preview.insert(c.key(), Load::Loading);
            }
            missing
        });
        for reference in missing {
            let key = reference.key();
            spawn(
                async move {
                    addons::catalog(&reference.addon, &reference.catalog.kind, &reference.catalog.id, &[]).await
                },
                move |app, result| {
                    app.spotlight_preview.write(|preview| {
                        preview.insert(key, match result {
                            Ok(items) => Load::Ready(items),
                            Err(error) => Load::Failed(error),
                        });
                    });
                },
            );
        }
        self.spotlight_picker.set(true);
    }

    pub fn close_spotlight_picker(&self) {
        if self.spotlight_picker.get_untracked() {
            self.spotlight_picker.set(false);
        }
    }

    pub fn load_board(&self) {
        let (show_spotlight, spotlight_rows) =
            self.profile.peek(|p| (p.settings.show_spotlight, p.settings.spotlight_rows_on_board));
        let catalogs = self.spotlight_catalogs();
        let spotlight = if show_spotlight { self.spotlight_keys() } else { Vec::new() };
        let mut refs: Vec<CatalogRef> = catalogs.iter().take(40).cloned().collect();
        // A Spotlight catalog past the Board's first 40 is fetched too.
        for key in &spotlight {
            if !refs.iter().any(|r| &r.key() == key)
                && let Some(reference) = catalogs.iter().find(|c| &c.key() == key)
            {
                refs.push(reference.clone());
            }
        }
        let rows: Vec<CatalogRow> = refs.iter().enumerate().map(|(index, reference)| {
            let mut row = Self::row_for(reference);
            let featured = spotlight.contains(&row.key);
            row.on_board = index < 40 && !(featured && !spotlight_rows);
            row
        }).collect();
        let generation = self.board.write(|board| {
            board.generation += 1;
            board.rows = rows;
            board.spotlight = spotlight.clone();
            board.featured.clear();
            board.carousel.select(0);
            board.carousel.hover(false);
            board.generation
        });
        self.schedule_featured_rotation(generation);
        for reference in refs {
            let key = reference.key();
            spawn(
                async move {
                    addons::catalog(
                        &reference.addon,
                        &reference.catalog.kind,
                        &reference.catalog.id,
                        &[],
                    )
                    .await
                },
                move |app, result| {
                    app.board.write(|board| {
                        if board.generation != generation {
                            return;
                        }
                        if let Some(row) = board.rows.iter_mut().find(|r| r.key == key) {
                            row.items = match result {
                                Ok(items) => Load::Ready(items),
                                Err(error) => Load::Failed(error),
                            };
                        }
                        if board.spotlight.contains(&key) {
                            let current = board.featured.get(board.carousel.index.get_untracked())
                                .map(|(meta, _)| (meta.kind.clone(), meta.id().to_owned()));
                            board.featured = featured_titles(&board.rows, &board.spotlight);
                            // Keep the slide on screen while later catalogs slot in.
                            if let Some(index) = current.and_then(|current| board.featured.iter()
                                .position(|(meta, _)| (meta.kind.clone(), meta.id().to_owned()) == current))
                                && index != board.carousel.index.get_untracked()
                            {
                                board.carousel.index.set(index);
                            }
                        }
                    });
                },
            );
        }
        // Catalog selections may no longer exist after an addon change.
        self.discover.write(|d| {
            d.kind.clear();
            d.catalog_key.clear();
            d.items.clear();
        });
    }

    fn schedule_featured_rotation(&self, generation: u64) {
        let active = self.current_route() == Route::Board
            && self.controllers.board.offset() < 300.0
            && !incular::animation::disable_animations()
            && self.board.peek(|board| !board.carousel.hovered.get()
                && !board.carousel.paused.get_untracked());
        let delay = std::time::Duration::from_millis(if active { 33 } else { 250 });
        spawn(
            async move { tokio::time::sleep(delay).await },
            move |app, ()| {
                let Some(carousel) = app.board.peek(|board| {
                    (board.generation == generation).then(|| board.carousel.clone())
                }) else { return };
                let on_screen = app.current_route() == Route::Board && app.controllers.board.offset() < 300.0;
                if carousel.tick(std::time::Instant::now(), on_screen) {
                    let count = app.board.peek(|board| board.featured.len());
                    if count > 1 {
                        carousel.advance(1, count);
                    } else {
                        carousel.reset_timer();
                    }
                }
                app.schedule_featured_rotation(generation);
            },
        );
    }

    // ----------------------------------------------------------- discover

    /// Catalogs Discover can show: nothing required beyond a genre.
    pub fn discover_catalogs(&self) -> Vec<CatalogRef> {
        let addons = self.profile.peek(|p| p.addons.clone());
        addons::all_catalogs(&addons)
            .into_iter()
            .filter(|c| {
                c.catalog
                    .extras()
                    .iter()
                    .all(|e| !e.is_required || (e.name == "genre" && !e.options.is_empty()))
            })
            .collect()
    }

    pub fn ensure_discover(&self) {
        if self.discover.peek(|d| !d.catalog_key.is_empty()) {
            return;
        }
        let catalogs = self.discover_catalogs();
        let Some(first) = catalogs.first() else { return };
        self.select_discover(first.catalog.kind.clone(), first.key(), None);
    }

    pub fn select_discover_kind(&self, kind: String) {
        let catalogs = self.discover_catalogs();
        if let Some(first) = catalogs.iter().find(|c| c.catalog.kind == kind) {
            self.select_discover(kind, first.key(), None);
        }
    }

    pub fn select_discover(&self, kind: String, catalog_key: String, genre: Option<String>) {
        let reference = self
            .discover_catalogs()
            .into_iter()
            .find(|c| c.key() == catalog_key);
        let genre = match (&reference, genre) {
            (Some(r), None) if r.catalog.requires_extra("genre") => {
                r.catalog.genre_options().first().cloned()
            }
            (_, genre) => genre,
        };
        self.discover.write(|d| {
            d.kind = kind;
            d.catalog_key = catalog_key;
            d.genre = genre;
            d.items.clear();
            d.exhausted = false;
            d.error = None;
            d.loading = false;
            d.generation += 1;
        });
        self.controllers.discover.jump_to(0.0);
        self.load_more_discover();
    }

    pub fn load_more_discover(&self) {
        let (key, genre, skip, generation, busy) = self.discover.peek(|d| {
            (
                d.catalog_key.clone(),
                d.genre.clone(),
                d.items.len(),
                d.generation,
                d.loading || d.exhausted,
            )
        });
        if busy || key.is_empty() {
            return;
        }
        let Some(reference) = self.discover_catalogs().into_iter().find(|c| c.key() == key)
        else {
            return;
        };
        let mut extra = Vec::new();
        if let Some(genre) = genre {
            extra.push(("genre".to_owned(), genre));
        }
        let supports_skip = reference.catalog.supports_extra("skip");
        if skip > 0 {
            if !supports_skip {
                self.discover.write(|d| d.exhausted = true);
                return;
            }
            extra.push(("skip".to_owned(), skip.to_string()));
        }
        self.discover.write(|d| d.loading = true);
        spawn(
            async move {
                addons::catalog(
                    &reference.addon,
                    &reference.catalog.kind,
                    &reference.catalog.id,
                    &extra,
                )
                .await
            },
            move |app, result| {
                app.discover.write(|d| {
                    if d.generation != generation {
                        return;
                    }
                    d.loading = false;
                    match result {
                        Ok(items) => {
                            let before = d.items.len();
                            for item in items {
                                if !d.items.iter().any(|m| m.id == item.id) {
                                    d.items.push(item);
                                }
                            }
                            if d.items.len() == before || !supports_skip {
                                d.exhausted = true;
                            }
                        }
                        Err(error) => {
                            d.exhausted = true;
                            if d.items.is_empty() {
                                d.error = Some(error);
                            }
                        }
                    }
                });
            },
        );
    }

    // ------------------------------------------------------------- search

    /// Search-as-you-type: runs once typing pauses.
    pub fn search_typed(&self, text: String) {
        let ticket = self.search_typing.get() + 1;
        self.search_typing.set(ticket);
        let text = text.trim().to_owned();
        if text.is_empty() {
            self.run_search(String::new());
            return;
        }
        if text.chars().count() < 2 {
            return;
        }
        spawn(
            async { tokio::time::sleep(std::time::Duration::from_millis(450)).await },
            move |app, ()| {
                let current = app.search.peek(|s| s.query.clone());
                if app.search_typing.get() == ticket && current != text {
                    app.run_search(text);
                }
            },
        );
    }

    pub fn open_search_menu(&self) {
        let idle = matches!(*self.search_feed.borrow(), Load::Idle | Load::Failed(_));
        if idle {
            *self.search_feed.borrow_mut() = Load::Loading;
            spawn(crate::suggest::load(), |app, entries| {
                *app.search_feed.borrow_mut() = match entries {
                    Some(entries) => Load::Ready(Rc::new(entries)),
                    None => Load::Failed("unavailable".into()),
                };
                app.refresh_suggestions(&app.controllers.search.text());
            });
        }
        self.refresh_suggestions(&self.controllers.search.text());
        if !self.search_menu.get_untracked() {
            self.search_menu.set(true);
        }
    }

    pub fn close_search_menu(&self) {
        if self.search_menu.get_untracked() {
            self.search_menu.set(false);
        }
    }

    /// Typing in a search field: open the dropdown and match again.
    pub fn search_menu_typed(&self, text: &str) {
        self.refresh_suggestions(text);
        if !self.search_menu.get_untracked() {
            self.open_search_menu();
        }
    }

    fn refresh_suggestions(&self, text: &str) {
        let found = match &*self.search_feed.borrow() {
            Load::Ready(entries) => crate::suggest::suggest(entries, text, 10),
            _ => Vec::new(),
        };
        if self.search_menu_query.get_untracked() != text {
            self.search_menu_query.set(text.to_owned());
        }
        if self.search_suggestions.get_untracked() != found {
            self.search_suggestions.set(found);
        }
    }

    /// A history entry or suggestion picked from the dropdown.
    pub fn pick_search(&self, query: String) {
        self.submit_search(query);
        if self.current_route() != Route::Search {
            self.go(Route::Search);
        }
    }

    /// An explicit search (Enter, button, recent chip): also remembered.
    pub fn submit_search(&self, query: String) {
        self.close_search_menu();
        let query = query.trim().to_owned();
        self.search_typing.set(self.search_typing.get() + 1);
        if self.controllers.search.text() != query {
            self.controllers.search.set_text(&query);
        }
        self.remember_search(&query);
        if self.search.peek(|s| s.query != query) {
            self.run_search(query);
        }
    }

    pub fn remember_search(&self, query: &str) {
        let query = query.trim();
        if query.chars().count() < 2 {
            return;
        }
        self.update_settings(|s| {
            s.recent_searches.retain(|q| !q.eq_ignore_ascii_case(query));
            s.recent_searches.insert(0, query.to_owned());
            s.recent_searches.truncate(8);
        });
    }

    pub fn clear_recent_searches(&self) {
        self.update_settings(|s| s.recent_searches.clear());
    }

    pub fn run_search(&self, query: String) {
        let query = query.trim().to_owned();
        let addons = self.profile.peek(|p| p.addons.clone());
        let refs: Vec<CatalogRef> = if query.is_empty() {
            Vec::new()
        } else {
            addons::all_catalogs(&addons)
                .into_iter()
                .filter(|c| {
                    c.catalog.supports_extra("search")
                        && c.catalog
                            .extras()
                            .iter()
                            .all(|e| !e.is_required || e.name == "search")
                })
                .collect()
        };
        let generation = self.search.write(|s| {
            s.generation += 1;
            s.query = query.clone();
            s.rows = refs.iter().map(Self::row_for).collect();
            s.generation
        });
        self.controllers.search_results.jump_to(0.0);
        for reference in refs {
            let key = reference.key();
            let extra = vec![("search".to_owned(), query.clone())];
            spawn(
                async move {
                    addons::catalog(
                        &reference.addon,
                        &reference.catalog.kind,
                        &reference.catalog.id,
                        &extra,
                    )
                    .await
                },
                move |app, result| {
                    app.search.write(|s| {
                        if s.generation != generation {
                            return;
                        }
                        if let Some(row) = s.rows.iter_mut().find(|r| r.key == key) {
                            row.items = match result {
                                Ok(items) => Load::Ready(items),
                                Err(error) => Load::Failed(error),
                            };
                        }
                    });
                },
            );
        }
    }

    // ------------------------------------------------------------- detail

    pub fn open_detail(&self, preview: Meta) {
        let kind = preview.kind.clone();
        let id = preview.id().to_owned();
        if id.is_empty() {
            return;
        }
        self.player.pending_binge.borrow_mut().take();
        self.close_season_picker();
        // Opening a result is what makes a typed query worth remembering.
        if self.route.get_untracked().first() == Some(&Route::Search) {
            let query = self.search.peek(|s| s.query.clone());
            self.remember_search(&query);
        }
        let generation = self.detail.write(|d| {
            d.generation += 1;
            d.quality = Quality::All;
            d.kind = kind.clone();
            d.id = id.clone();
            d.preview = Some(preview);
            d.meta = Load::Loading;
            d.season = None;
            d.video = None;
            d.streams.clear();
            d.generation
        });
        self.controllers.detail.jump_to(0.0);
        self.controllers.streams.jump_to(0.0);
        self.push(Route::Detail);
        let addons = self.profile.peek(|p| p.addons.clone());
        spawn(
            async move { addons::meta(&addons, &kind, &id).await },
            move |app, result| app.detail_loaded(generation, result),
        );
    }

    fn detail_loaded(&self, generation: u64, result: Result<Meta, String>) {
        if self.detail.peek(|d| d.generation) != generation {
            return;
        }
        match result {
            Ok(meta) => {
                let resume = self
                    .library
                    .peek(|l| l.get(meta.id()).and_then(|i| i.state.video_id.clone()));
                let resume_video = resume
                    .and_then(|id| meta.videos.iter().find(|v| v.id() == id).cloned());
                let seasons = meta.seasons();
                let season = resume_video
                    .as_ref()
                    .and_then(|v| v.season)
                    .or_else(|| seasons.first().copied());
                let single = if meta.videos.len() <= 1 {
                    Some(meta.videos.first().cloned().unwrap_or_else(|| Video {
                        id: Some(
                            meta.behavior_hints
                                .default_video_id
                                .clone()
                                .unwrap_or_else(|| meta.id().to_owned()),
                        ),
                        ..Video::default()
                    }))
                } else {
                    meta.behavior_hints
                        .default_video_id
                        .as_ref()
                        .and_then(|id| meta.videos.iter().find(|v| v.id() == id).cloned())
                };
                self.detail.write(|d| {
                    d.meta = Load::Ready(meta);
                    d.season = season;
                });
                if let Some(video) = single {
                    self.select_video(video);
                }
            }
            Err(error) => {
                // Addons can still serve streams for an id nobody describes.
                let fallback = self.detail.peek(|d| {
                    (d.kind != "series").then(|| Video {
                        id: Some(d.id.clone()),
                        ..Video::default()
                    })
                });
                self.detail.write(|d| d.meta = Load::Failed(error));
                if let Some(video) = fallback {
                    self.select_video(video);
                }
            }
        }
    }

    pub fn select_season(&self, season: u32) {
        self.close_season_picker();
        self.detail.write(|d| d.season = Some(season));
        self.controllers.streams.jump_to(0.0);
    }

    pub fn toggle_season_picker(&self) {
        let open = !self.season_open.get_untracked();
        if open {
            // The dropdown is placed under the trigger at the list's top.
            self.controllers.streams.jump_to(0.0);
            self.controllers.season_search.set_text("");
            self.season_filter.set(String::new());
            self.controllers.season_list.jump_to(0.0);
        }
        self.season_open.set(open);
    }

    pub fn close_season_picker(&self) {
        if self.season_open.get_untracked() {
            self.season_open.set(false);
        }
    }

    pub fn filter_seasons(&self, query: String) {
        self.season_filter.set(query);
        self.controllers.season_list.jump_to(0.0);
    }

    pub fn clear_video(&self) {
        self.player.pending_binge.borrow_mut().take();
        self.detail.write(|d| {
            d.video = None;
            d.streams.clear();
            d.generation += 1;
        });
    }

    pub fn select_video(&self, video: Video) {
        let kind = self.detail.peek(|d| d.kind.clone());
        let video_id = video.id().to_owned();
        let addons: Vec<Descriptor> = self.profile.peek(|p| {
            p.addons
                .iter()
                .filter(|a| a.manifest.supports("stream", &kind, &video_id))
                .cloned()
                .collect()
        });
        let embedded = video.streams.clone();
        let generation = self.detail.write(|d| {
            d.generation += 1;
            d.video = Some(video);
            d.streams = addons
                .iter()
                .map(|a| StreamGroup {
                    addon_name: a.manifest.name.clone(),
                    items: Load::Loading,
                })
                .collect();
            if !embedded.is_empty() {
                d.streams.insert(
                    0,
                    StreamGroup {
                        addon_name: "Included".to_owned(),
                        items: Load::Ready(embedded),
                    },
                );
            }
            d.generation
        });
        self.controllers.streams.jump_to(0.0);
        for addon_descriptor in addons {
            let name = addon_descriptor.manifest.name.clone();
            let (kind, video_id) = (kind.clone(), video_id.clone());
            spawn(
                async move { addons::streams(&addon_descriptor, &kind, &video_id).await },
                move |app, result| {
                    if app.detail.peek(|d| d.generation) != generation {
                        return;
                    }
                    app.detail.write(|d| {
                        if let Some(group) = d
                            .streams
                            .iter_mut()
                            .find(|g| g.addon_name == name && g.items == Load::Loading)
                        {
                            group.items = match result {
                                Ok(items) => Load::Ready(items),
                                Err(error) => Load::Failed(error),
                            };
                        }
                    });
                    app.try_binge();
                },
            );
        }
        self.try_binge();
    }

    // ------------------------------------------------------------ library

    pub fn in_library(&self, id: &str) -> bool {
        self.library
            .read(|l| l.get(id).is_some_and(|i| !i.removed))
    }

    pub fn toggle_library(&self, meta: &Meta) {
        let id = meta.id().to_owned();
        if id.is_empty() {
            return;
        }
        let now = now_iso();
        let added = self.library.write(|library| {
            let item = library.entry(id.clone()).or_insert_with(|| LibraryItem {
                id: id.clone(),
                removed: true,
                ctime: Some(now.clone()),
                ..LibraryItem::default()
            });
            item.name = meta.title().to_owned();
            item.kind = meta.kind.clone();
            item.poster = meta.poster.clone();
            item.poster_shape = meta.poster_shape.clone();
            item.removed = !item.removed;
            item.temp = false;
            item.mtime = Some(now.clone());
            !item.removed
        });
        self.library.peek(store::save_library);
        self.notify(if added {
            "Added to your library"
        } else {
            "Removed from your library"
        });
    }

    pub fn dismiss_continue_watching(&self, id: &str) {
        let now = now_iso();
        self.library.write(|library| {
            if let Some(item) = library.get_mut(id) {
                item.state.time_offset = 0.0;
                item.mtime = Some(now);
            }
        });
        self.library.peek(store::save_library);
    }

    /// Records playback progress. `ended` advances series to `next`.
    pub fn record_progress(
        &self,
        meta: &Meta,
        video_id: &str,
        position_ms: f64,
        duration_ms: f64,
        watched_delta_ms: f64,
        ended: bool,
        next: Option<&Video>,
        push: bool,
    ) {
        let id = meta.id().to_owned();
        if id.is_empty() {
            return;
        }
        let now = now_iso();
        self.library.write_silent(|library| {
            let item = library.entry(id.clone()).or_insert_with(|| LibraryItem {
                id: id.clone(),
                name: meta.title().to_owned(),
                kind: meta.kind.clone(),
                poster: meta.poster.clone(),
                poster_shape: meta.poster_shape.clone(),
                // Watched but not added to the library: kept, but temporary.
                removed: true,
                temp: true,
                ctime: Some(now.clone()),
                ..LibraryItem::default()
            });
            let state = &mut item.state;
            if state.video_id.as_deref() != Some(video_id) {
                state.time_watched = 0.0;
            }
            state.video_id = Some(video_id.to_owned());
            state.time_offset = position_ms.max(1.0);
            state.duration = duration_ms;
            state.time_watched += watched_delta_ms;
            state.overall_time_watched += watched_delta_ms;
            state.last_watched = Some(now.clone());
            if ended {
                state.times_watched += 1.0;
                state.flagged_watched = 1.0;
                match next {
                    Some(next) => {
                        state.video_id = Some(next.id().to_owned());
                        state.time_offset = 1.0;
                        state.time_watched = 0.0;
                        state.duration = 0.0;
                    }
                    None => state.time_offset = 0.0,
                }
            }
            item.mtime = Some(now.clone());
        });
        if push {
            self.library.write(|_| {});
            self.library.peek(store::save_library);
        }
    }

    // ------------------------------------------------------------- addons

    fn addons_changed(&self) {
        self.save_profile();
        self.load_board();
    }

    pub fn install_addon_from_input(&self) {
        let input = self.controllers.addon_url.text();
        match addons::normalize_transport_url(&input) {
            Some(url) => self.install_addon(url),
            None => self.notify("Paste an addon URL first"),
        }
    }

    pub fn install_addon(&self, transport_url: String) {
        self.addons.write(|a| a.busy = true);
        spawn(
            async move { addons::fetch_manifest(&transport_url).await },
            |app, result| {
                app.addons.write(|a| a.busy = false);
                match result {
                    Ok(descriptor) => app.add_addon(descriptor),
                    Err(error) => app.notify(format!("Could not install addon: {error}")),
                }
            },
        );
    }

    pub fn add_addon(&self, descriptor: Descriptor) {
        let name = descriptor.manifest.name.clone();
        self.profile.write(|p| {
            match p.addons.iter_mut().find(|a| {
                a.transport_url == descriptor.transport_url
                    || a.manifest.id == descriptor.manifest.id
            }) {
                Some(existing) => {
                    let flags = existing.flags.clone();
                    *existing = descriptor;
                    existing.flags = flags;
                }
                None => p.addons.push(descriptor),
            }
        });
        self.controllers.addon_url.set_text("");
        self.addons_changed();
        self.notify(format!("{name} installed"));
    }

    pub fn uninstall_addon(&self, transport_url: &str) {
        let removed = self.profile.write(|p| {
            let before = p.addons.len();
            p.addons
                .retain(|a| a.transport_url != transport_url || a.flags.protected);
            before != p.addons.len()
        });
        if removed {
            self.addons_changed();
            self.notify("Addon removed");
        } else {
            self.notify("This addon is protected and cannot be removed");
        }
    }

    /// Moves an addon earlier (-1) or later (+1); order decides priority.
    pub fn move_addon(&self, transport_url: &str, delta: i32) {
        let moved = self.profile.write(|p| {
            let Some(index) = p.addons.iter().position(|a| a.transport_url == transport_url)
            else {
                return false;
            };
            let target = index as i32 + delta;
            if target < 0 || target as usize >= p.addons.len() {
                return false;
            }
            p.addons.swap(index, target as usize);
            true
        });
        if moved {
            self.addons_changed();
        }
    }

    pub fn ensure_community(&self) {
        if self.addons.peek(|a| a.community != Load::Idle) {
            return;
        }
        self.addons.write(|a| a.community = Load::Loading);
        spawn(addons::community_addons(), |app, result| {
            app.addons.write(|a| {
                a.community = match result {
                    Ok(list) => Load::Ready(list),
                    Err(error) => Load::Failed(error),
                }
            });
        });
    }

    // ------------------------------------------------------------- streaming

    /// Starts the built-in torrent engine (also the Settings "Retry").
    pub fn start_streaming(&self) {
        tracing::info!("Starting torrent engine");
        let settings = self.profile.peek(|p| p.settings.torrent.clone());
        crate::streaming::configure(&settings);
        spawn(crate::streaming::start(), |app, result| {
            if let Err(error) = &result {
                tracing::error!(%error, "Torrent engine startup failed");
                app.notify(format!("Torrent engine: {error}"));
            }
            tracing::info!(success = result.is_ok(), "Torrent engine startup completed");
            app.server_online.set(Some(result.is_ok()));
        });
    }

    pub fn update_settings(&self, update: impl FnOnce(&mut store::Settings)) {
        let (dir, torrent, subtitle_loading, sync_changed) = self.profile.write(|p| {
            let previous = p.settings.fast_subtitles;
            let old_sync = (p.settings.local_ai_enabled, p.settings.automatic_subtitle_sync);
            update(&mut p.settings);
            (p.settings.download_dir.clone(), p.settings.torrent.clone(),
                (previous != p.settings.fast_subtitles).then_some(p.settings.fast_subtitles),
                old_sync != (p.settings.local_ai_enabled, p.settings.automatic_subtitle_sync))
        });
        crate::downloads::set_custom_folder(&dir);
        crate::streaming::configure(&torrent);
        if let Some(fast) = subtitle_loading {
            self.player.set_subtitle_loading(fast);
        }
        if !self.profile.peek(|p| p.settings.local_ai_enabled) {
            self.ai.cancel();
            self.player.ai_match.set(None);
        }
        if sync_changed {
            self.auto_sync_preferences_changed();
        }
        self.save_profile();
    }
}
