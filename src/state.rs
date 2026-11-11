//! Application state and the actions that mutate it.
//!
//! All state lives on the UI thread. Background work (network, decoding)
//! runs on Tokio or plain threads and comes back through [`ui`], which
//! re-enters the UI thread and hands the callback the [`App`].

use crate::player::PlayerUi;
use crate::store::{self, Profile};
use crate::media::addon::{self, CatalogRef};
use crate::media::types::*;
use crate::media::{api, server};
use incular::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
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
    Detail,
    Player,
}

#[derive(Clone, Debug)]
pub struct CatalogRow {
    pub key: String,
    pub title: String,
    pub addon_name: String,
    pub items: Load<Vec<Meta>>,
}

#[derive(Default)]
pub struct BoardState {
    pub rows: Vec<CatalogRow>,
    pub generation: u64,
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

#[derive(Default)]
pub struct AccountState {
    pub busy: bool,
    pub error: Option<String>,
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
    pub email: TextEditingController,
    pub password: TextEditingController,
    pub server_url: TextEditingController,
    pub player_path: TextEditingController,
    pub download_dir: TextEditingController,
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
            email: TextEditingController::new(),
            password: TextEditingController::new(),
            server_url: TextEditingController::with_text(&profile.settings.server_url),
            player_path: TextEditingController::with_text(&profile.settings.custom_player_path),
            download_dir: TextEditingController::with_text(&profile.settings.download_dir),
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
    /// Season combobox on the detail page. Separate signals so typing in
    /// its search field rebuilds only the result list, not the field.
    pub season_open: Signal<bool>,
    pub season_filter: Signal<String>,
    pub addons: Shared<AddonsState>,
    pub account: Shared<AccountState>,
    pub server_online: Signal<Option<bool>>,
    pub server_settings: Signal<Load<server::ServerSettings>>,
    /// False while the startup screen is up.
    pub booted: Signal<bool>,
    search_typing: Rc<std::cell::Cell<u64>>,
    pub player: PlayerUi,
    pub downloads: Shared<Vec<crate::downloads::Download>>,
    pub controllers: Controllers,
}

impl App {
    pub fn new() -> Self {
        let profile = store::load_profile();
        let controllers = Controllers::new(&profile);
        crate::downloads::set_custom_folder(&profile.settings.download_dir);
        Self {
            route: Signal::new(vec![Route::Board]),
            toast: Signal::new(None),
            toast_generation: Rc::new(RefCell::new(0)),
            player: PlayerUi::new(&profile.settings),
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
            season_open: Signal::new(false),
            season_filter: Signal::new(String::new()),
            addons: Shared::new(AddonsState {
                tab: AddonsTab::Installed,
                community: Load::Idle,
                filter: String::new(),
                busy: false,
            }),
            account: Shared::new(AccountState::default()),
            server_online: Signal::new(None),
            server_settings: Signal::new(Load::Idle),
            booted: Signal::new(false),
            search_typing: Rc::new(std::cell::Cell::new(0)),
            downloads: Shared::new(crate::downloads::load()),
            controllers,
        }
    }

    // ------------------------------------------------------------ startup

    pub fn startup(&self) {
        let has_addons = self.profile.peek(|p| !p.addons.is_empty());
        if has_addons {
            self.load_board();
        } else {
            spawn(addon::default_addons(), |app, addons| {
                if addons.is_empty() {
                    app.notify("Could not reach the default addons. Check your connection.");
                    app.finish_boot();
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
        self.sync_account();
        self.ensure_server(false);
        // Never hold the startup screen hostage to a slow network.
        spawn(
            async { tokio::time::sleep(std::time::Duration::from_secs(8)).await },
            |app, ()| app.finish_boot(),
        );
    }

    pub fn finish_boot(&self) {
        if !self.booted.get_untracked() {
            self.booted.set(true);
        }
    }

    /// Leaves the startup screen once enough of the board has arrived.
    fn check_boot(&self) {
        if self.booted.get_untracked() || std::env::var_os("SPOUTPEAK_HOLD_SPLASH").is_some() {
            return;
        }
        let (done, total) = self.board.peek(|b| {
            (
                b.rows.iter().filter(|r| r.items != Load::Loading).count(),
                b.rows.len(),
            )
        });
        if total == 0 || done >= total.min(3) {
            self.finish_boot();
        }
    }

    pub fn save_profile(&self) {
        self.profile.peek(store::save_profile);
    }

    pub fn notify(&self, message: impl Into<String>) {
        let generation = {
            let mut generation = self.toast_generation.borrow_mut();
            *generation += 1;
            *generation
        };
        self.toast.set(Some(message.into()));
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
        if std::env::var_os("SPOUTPEAK_DEBUG").is_some() {
            eprintln!("[nav] go {route:?}");
        }
        self.leave_player_if_needed();
        self.route.set(vec![route]);
        self.controllers.focus.request_focus();
        match route {
            Route::Discover => self.ensure_discover(),
            Route::Addons => self.ensure_community(),
            Route::Settings => self.load_server_settings(),
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
        }
    }

    pub fn load_board(&self) {
        let addons = self.profile.peek(|p| p.addons.clone());
        let refs: Vec<CatalogRef> = addon::all_catalogs(&addons)
            .into_iter()
            .filter(|c| c.catalog.is_browsable())
            .take(40)
            .collect();
        let generation = self.board.write(|board| {
            board.generation += 1;
            board.rows = refs.iter().map(Self::row_for).collect();
            board.generation
        });
        for reference in refs {
            let key = reference.key();
            spawn(
                async move {
                    addon::catalog(
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
                    });
                    app.check_boot();
                },
            );
        }
        self.check_boot();
        // Catalog selections may no longer exist after an addon change.
        self.discover.write(|d| {
            d.kind.clear();
            d.catalog_key.clear();
            d.items.clear();
        });
    }

    // ----------------------------------------------------------- discover

    /// Catalogs Discover can show: nothing required beyond a genre.
    pub fn discover_catalogs(&self) -> Vec<CatalogRef> {
        let addons = self.profile.peek(|p| p.addons.clone());
        addon::all_catalogs(&addons)
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
                addon::catalog(
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

    /// An explicit search (Enter, button, recent chip): also remembered.
    pub fn submit_search(&self, query: String) {
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
            addon::all_catalogs(&addons)
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
                    addon::catalog(
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
            async move { addon::meta(&addons, &kind, &id).await },
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
                async move { addon::streams(&addon_descriptor, &kind, &video_id).await },
                move |app, result| {
                    if app.detail.peek(|d| d.generation) != generation {
                        return;
                    }
                    let found = result.as_ref().ok().cloned().unwrap_or_default();
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
                    app.try_binge(&found);
                },
            );
        }
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
        let (item, added) = self.library.write(|library| {
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
            (item.clone(), !item.removed)
        });
        self.library.peek(store::save_library);
        self.push_library(vec![item]);
        self.notify(if added {
            "Added to your library"
        } else {
            "Removed from your library"
        });
    }

    pub fn dismiss_continue_watching(&self, id: &str) {
        let now = now_iso();
        let item = self.library.write(|library| {
            library.get_mut(id).map(|item| {
                item.state.time_offset = 0.0;
                item.mtime = Some(now);
                item.clone()
            })
        });
        self.library.peek(store::save_library);
        if let Some(item) = item {
            self.push_library(vec![item]);
        }
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
        let item = self.library.write_silent(|library| {
            let item = library.entry(id.clone()).or_insert_with(|| LibraryItem {
                id: id.clone(),
                name: meta.title().to_owned(),
                kind: meta.kind.clone(),
                poster: meta.poster.clone(),
                poster_shape: meta.poster_shape.clone(),
                // Like Media: watched-but-not-added items are temporary.
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
            item.clone()
        });
        if push {
            self.library.write(|_| {});
            self.library.peek(store::save_library);
            self.push_library(vec![item]);
        }
    }

    fn auth_key(&self) -> Option<String> {
        self.profile.peek(|p| p.auth.as_ref().map(|a| a.key.clone()))
    }

    fn push_library(&self, items: Vec<LibraryItem>) {
        let Some(key) = self.auth_key() else { return };
        spawn(
            async move { api::library_put(&key, &items).await },
            |app, result| {
                if let Err(error) = result {
                    app.notify(format!("Library sync failed: {error}"));
                }
            },
        );
    }

    // ------------------------------------------------------------ account

    /// Pulls addons and library for the signed-in user and merges them.
    pub fn sync_account(&self) {
        let Some(key) = self.auth_key() else { return };
        let addons_key = key.clone();
        spawn(
            async move { api::addon_collection_get(&addons_key).await },
            |app, result| match result {
                Ok(addons) if !addons.is_empty() => {
                    let changed = app.profile.peek(|p| p.addons != addons);
                    if changed {
                        app.profile.write(|p| p.addons = addons);
                        app.save_profile();
                        app.load_board();
                    }
                }
                Ok(_) => {}
                Err(error) => app.notify(format!("Addon sync failed: {error}")),
            },
        );
        spawn(
            async move { api::library_get_all(&key).await },
            |app, result| match result {
                Ok(remote) => app.merge_library(remote),
                Err(error) => app.notify(format!("Library sync failed: {error}")),
            },
        );
    }

    fn merge_library(&self, remote: Vec<LibraryItem>) {
        let remote_ids: std::collections::HashSet<String> =
            remote.iter().map(|i| i.id.clone()).collect();
        let newer_locally = self.library.write(|library| {
            for item in remote {
                let keep_local = library
                    .get(&item.id)
                    .is_some_and(|local| local.mtime > item.mtime);
                if !keep_local {
                    library.insert(item.id.clone(), item);
                }
            }
            library
                .values()
                .filter(|local| !remote_ids.contains(&local.id))
                .cloned()
                .collect::<Vec<_>>()
        });
        self.library.peek(store::save_library);
        self.push_library(newer_locally);
    }

    pub fn sign_in(&self, register: bool) {
        let email = self.controllers.email.text().trim().to_owned();
        let password = self.controllers.password.text();
        if email.is_empty() || password.is_empty() {
            self.account
                .write(|a| a.error = Some("Enter your email and password.".into()));
            return;
        }
        self.account.write(|a| {
            a.busy = true;
            a.error = None;
        });
        spawn(
            async move {
                if register {
                    api::register(&email, &password).await
                } else {
                    api::login(&email, &password).await
                }
            },
            |app, result| {
                match result {
                    Ok(auth) => {
                        app.controllers.password.set_text("");
                        app.account.write(|a| a.busy = false);
                        app.notify(format!("Signed in as {}", auth.user.email));
                        app.profile.write(|p| p.auth = Some(auth));
                        app.save_profile();
                        app.sync_account();
                    }
                    Err(error) => app.account.write(|a| {
                        a.busy = false;
                        a.error = Some(error);
                    }),
                };
            },
        );
    }

    pub fn sign_out(&self) {
        if let Some(key) = self.auth_key() {
            spawn(async move { api::logout(&key).await }, |_, _| {});
        }
        self.profile.write(|p| {
            p.auth = None;
            p.addons.clear();
        });
        self.library.write(|l| l.clear());
        self.library.peek(store::save_library);
        self.save_profile();
        self.notify("Signed out");
        spawn(addon::default_addons(), |app, addons| {
            app.profile.write(|p| p.addons = addons);
            app.save_profile();
            app.load_board();
        });
    }

    // ------------------------------------------------------------- addons

    fn addons_changed(&self) {
        self.save_profile();
        self.load_board();
        if let Some(key) = self.auth_key() {
            let addons = self.profile.peek(|p| p.addons.clone());
            spawn(
                async move { api::addon_collection_set(&key, &addons).await },
                |app, result| {
                    if let Err(error) = result {
                        app.notify(format!("Addon sync failed: {error}"));
                    }
                },
            );
        }
    }

    pub fn install_addon_from_input(&self) {
        let input = self.controllers.addon_url.text();
        match addon::normalize_transport_url(&input) {
            Some(url) => self.install_addon(url),
            None => self.notify("Paste an addon URL first"),
        }
    }

    pub fn install_addon(&self, transport_url: String) {
        self.addons.write(|a| a.busy = true);
        spawn(
            async move { addon::fetch_manifest(&transport_url).await },
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
        spawn(addon::community_addons(), |app, result| {
            app.addons.write(|a| {
                a.community = match result {
                    Ok(list) => Load::Ready(list),
                    Err(error) => Load::Failed(error),
                }
            });
        });
    }

    // ------------------------------------------------------------- server

    pub fn server_url(&self) -> String {
        self.profile.peek(|p| p.settings.server_url.clone())
    }

    pub fn check_server(&self) {
        let url = self.server_url();
        spawn(async move { server::is_online(&url).await }, |app, online| {
            app.server_online.set(Some(online));
        });
    }

    /// Checks the streaming server and, if it is local and not running,
    /// starts the built-in one (at startup only when autostart is on;
    /// `force` is the Settings page's Start button).
    pub fn ensure_server(&self, force: bool) {
        let url = self.server_url();
        let autostart = force || self.profile.peek(|p| p.settings.autostart_server);
        spawn(
            async move {
                if server::is_online(&url).await {
                    return Ok(());
                }
                if !autostart || !url.contains("127.0.0.1") && !url.contains("localhost") {
                    return Err(None);
                }
                crate::media::local_server::start(&url).await.map_err(Some)?;
                if server::is_online(&url).await { Ok(()) } else { Err(None) }
            },
            |app, result: Result<(), Option<String>>| {
                if let Err(Some(error)) = &result {
                    app.notify(format!("Streaming server: {error}"));
                }
                app.server_online.set(Some(result.is_ok()));
            },
        );
    }

    pub fn update_settings(&self, update: impl FnOnce(&mut store::Settings)) {
        let dir = self.profile.write(|p| {
            update(&mut p.settings);
            p.settings.download_dir.clone()
        });
        crate::downloads::set_custom_folder(&dir);
        self.save_profile();
    }

    // ------------------------------------------------- server settings

    /// Fetches the streaming server's own settings for the Settings page.
    pub fn load_server_settings(&self) {
        let url = self.server_url();
        self.server_settings.set(Load::Loading);
        spawn(async move { server::get_settings(&url).await }, |app, result| {
            app.server_settings.set(match result {
                Ok(settings) => Load::Ready(settings),
                Err(error) => Load::Failed(error),
            });
        });
    }

    /// Changes server settings and saves them; reloads to show what the
    /// server actually kept.
    pub fn update_server_settings(
        &self,
        change: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
    ) {
        let Load::Ready(mut settings) = self.server_settings.get_untracked() else {
            return;
        };
        change(&mut settings.values);
        let values = settings.values.clone();
        self.server_settings.set(Load::Ready(settings));
        let url = self.server_url();
        spawn(async move { server::set_settings(&url, values).await }, |app, result| {
            match result {
                Ok(()) => app.notify("Streaming server settings saved"),
                Err(error) => app.notify(error),
            }
            app.load_server_settings();
        });
    }
}
