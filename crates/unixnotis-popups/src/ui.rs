//! Popup UI state, layout, and event handling.

#[path = "icons/mod.rs"]
mod icons;
#[path = "ui_window.rs"]
mod ui_window;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::thread;

use gtk::prelude::*;
use gtk::Align;
use gtk::{gdk, glib};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, warn};
use unixnotis_core::{Config, NotificationView, Urgency};

use crate::dbus::{UiCommand, UiEvent};
use unixnotis_ui::css::{self, CssManager};

use icons::{
    collect_icon_candidates, decode_icon_file, file_path_from_hint, image_data_texture,
    resolve_icon_image, DesktopIconIndex, RasterIcon,
};
use ui_window::{apply_popup_config, build_popup_window};

const ICON_CACHE_MAX_ENTRIES: usize = 256;
const ICON_DECODE_WORKERS: usize = 2;

struct IconDecodeJob {
    path: PathBuf,
    reply: async_channel::Sender<Result<RasterIcon, String>>,
}

struct IconDecodePool {
    tx: async_channel::Sender<IconDecodeJob>,
}

impl IconDecodePool {
    fn global() -> &'static IconDecodePool {
        static POOL: OnceLock<IconDecodePool> = OnceLock::new();
        POOL.get_or_init(|| IconDecodePool::new(ICON_DECODE_WORKERS))
    }

    fn new(worker_count: usize) -> Self {
        let (tx, rx) = async_channel::unbounded::<IconDecodeJob>();
        // Limit decode concurrency to keep bursty icon loads from spawning unbounded threads.
        for idx in 0..worker_count.max(1) {
            let rx = rx.clone();
            let name = format!("unixnotis-icon-decode-{idx}");
            if thread::Builder::new().name(name).spawn(move || worker_loop(rx)).is_err() {
                warn!("failed to spawn icon decode worker");
            }
        }
        Self { tx }
    }

    fn submit(&self, path: PathBuf, reply: async_channel::Sender<Result<RasterIcon, String>>) {
        let _ = self.tx.send_blocking(IconDecodeJob { path, reply });
    }
}

fn worker_loop(rx: async_channel::Receiver<IconDecodeJob>) {
    while let Ok(job) = rx.recv_blocking() {
        // Decode file-backed icons off the GTK thread to keep animations smooth.
        let result = decode_icon_file(&job.path);
        let _ = job.reply.send_blocking(result);
    }
}

/// Popup-only GTK state for notification toasts.
pub struct UiState {
    config: Config,
    config_path: std::path::PathBuf,
    css: CssManager,
    command_tx: UnboundedSender<UiCommand>,
    popup_window: gtk::ApplicationWindow,
    popup_stack: gtk::Box,
    popups: HashMap<u32, PopupEntry>,
    popup_order: VecDeque<u32>,
    desktop_icons: DesktopIconIndex,
    icon_cache: HashMap<String, Option<String>>,
    // FIFO order used to cap icon cache growth.
    icon_cache_order: VecDeque<String>,
}

struct PopupEntry {
    revealer: gtk::Revealer,
    root: gtk::Box,
}

impl UiState {
    pub fn new(
        app: &gtk::Application,
        config: Config,
        config_path: std::path::PathBuf,
        command_tx: UnboundedSender<UiCommand>,
        css: CssManager,
    ) -> Self {
        let (popup_window, popup_stack) = build_popup_window(app, &config);

        Self {
            config,
            config_path,
            css,
            command_tx,
            popup_window,
            popup_stack,
            popups: HashMap::new(),
            popup_order: VecDeque::new(),
            desktop_icons: DesktopIconIndex::new(),
            icon_cache: HashMap::new(),
            icon_cache_order: VecDeque::new(),
        }
    }

    pub fn handle_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Seed { state, active } => {
                if state.dnd_enabled {
                    for notification in active {
                        if notification.urgency == Urgency::Critical as u8 {
                            self.add_popup(notification);
                        }
                    }
                } else {
                    for notification in active {
                        self.add_popup(notification);
                    }
                }
            }
            UiEvent::NotificationAdded(notification, show_popup) => {
                if show_popup {
                    debug!(
                        id = notification.id,
                        app = %notification.app_name,
                        "popup added"
                    );
                    self.add_popup(notification);
                }
            }
            UiEvent::NotificationUpdated(notification, show_popup) => {
                debug!(
                    id = notification.id,
                    app = %notification.app_name,
                    "popup updated"
                );
                self.replace_popup(notification, show_popup);
            }
            UiEvent::NotificationClosed(id, _reason) => {
                debug!(id, "popup closed");
                self.remove_popup(id);
            }
            UiEvent::StateChanged(state) => {
                if state.dnd_enabled {
                    debug!("clearing popups due to dnd");
                    self.clear_popups();
                }
            }
            UiEvent::CssReload => {
                debug!("popup css reload requested");
                self.css.reload(css::DEFAULT_CSS);
            }
            UiEvent::ConfigReload => {
                debug!("popup config reload requested");
                self.reload_config();
            }
        }
    }

    fn reload_config(&mut self) {
        let config = match Config::load_from_path(&self.config_path) {
            Ok(config) => config,
            Err(err) => {
                tracing::warn!(?err, "failed to reload config");
                return;
            }
        };
        let theme_base = self
            .config_path
            .parent()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                Config::default_config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
        let theme_paths = match config.resolve_theme_paths_from(&theme_base) {
            Ok(paths) => paths,
            Err(err) => {
                tracing::warn!(?err, "failed to resolve theme paths");
                return;
            }
        };

        self.config = config.clone();
        debug!("popup config reloaded");
        self.css.update_theme(theme_paths, config.theme.clone());
        self.css.reload(css::DEFAULT_CSS);
        apply_popup_config(&self.popup_window, &self.popup_stack, &config);
    }

    fn add_popup(&mut self, notification: NotificationView) {
        let id = notification.id;
        if self.popups.contains_key(&id) {
            return;
        }

        let entry = self.build_popup_entry(&notification);
        self.popup_stack.prepend(&entry.revealer);
        self.popups.insert(id, entry);
        self.popup_order.push_front(id);
        self.update_popup_visibility();
        debug!(id, total = self.popup_order.len(), "popup inserted");
    }

    fn replace_popup(&mut self, notification: NotificationView, show_popup: bool) {
        let id = notification.id;
        self.remove_popup(id);
        if show_popup {
            self.add_popup(notification);
        }
    }

    fn remove_popup(&mut self, id: u32) {
        if let Some(entry) = self.popups.remove(&id) {
            entry.revealer.set_reveal_child(false);
            let stack = self.popup_stack.clone();
            entry
                .revealer
                .connect_notify_local(Some("child-revealed"), move |revealer, _| {
                    if !revealer.is_child_revealed() && revealer.parent().is_some() {
                        stack.remove(revealer);
                    }
                });
        }
        self.popup_order.retain(|item| *item != id);
        self.update_popup_visibility();
        debug!(id, total = self.popup_order.len(), "popup removed");
    }

    fn clear_popups(&mut self) {
        let ids: Vec<u32> = self.popup_order.iter().copied().collect();
        for id in ids {
            self.remove_popup(id);
        }
    }

    fn update_popup_visibility(&self) {
        let max_visible = self.config.popups.max_visible;
        let stack_depth = 3; // Increased depth for better visual pile

        if max_visible == 0 {
            for entry in self.popups.values() {
                entry.root.set_visible(false);
                entry.revealer.set_reveal_child(false);
            }
            self.popup_window.set_visible(false);
            debug!("popups disabled by max_visible = 0");
            return;
        }

        if self.popup_order.is_empty() {
            self.popup_window.set_visible(false);
        } else {
            self.popup_window.set_visible(true);
        }

        for (index, id) in self.popup_order.iter().enumerate() {
            if let Some(entry) = self.popups.get(id) {
                // Clean up previous state classes
                entry.root.remove_css_class("unixnotis-popup-visible");
                entry.root.remove_css_class("unixnotis-popup-stacked");
                for i in 0..stack_depth {
                    entry
                        .root
                        .remove_css_class(&format!("unixnotis-popup-stacked-{}", i));
                }

                if index < max_visible {
                    // Fully visible notification
                    entry.root.set_visible(true);
                    entry.revealer.set_reveal_child(true);
                    entry.root.add_css_class("unixnotis-popup-visible");
                } else if index < max_visible + stack_depth {
                    // Stacked (pile) notification
                    let stack_idx = index - max_visible;
                    entry.root.set_visible(true);
                    entry.revealer.set_reveal_child(true);
                    entry.root.add_css_class("unixnotis-popup-stacked");
                    entry
                        .root
                        .add_css_class(&format!("unixnotis-popup-stacked-{}", stack_idx));
                } else {
                    // Hidden
                    entry.root.set_visible(false);
                    entry.revealer.set_reveal_child(false);
                }
            }
        }
        debug!(
            visible = self.popup_order.len().min(max_visible + stack_depth),
            total = self.popup_order.len(),
            "popup visibility updated"
        );
    }

    fn build_popup_entry(&mut self, notification: &NotificationView) -> PopupEntry {
        let revealer = gtk::Revealer::new();
        revealer.add_css_class("unixnotis-popup-revealer");
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        revealer.set_transition_duration(200);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.add_css_class("unixnotis-popup-card");
        if notification.urgency == Urgency::Critical as u8 {
            root.add_css_class("critical");
        }

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.add_css_class("unixnotis-popup-header-row");
        if let Some(icon) = self.build_image_widget(notification) {
            icon.set_valign(Align::Center);
            icon.set_halign(Align::Start);
            icon.add_css_class("unixnotis-popup-icon");
            header.append(&icon);
        }
        let app = gtk::Label::new(Some(&notification.app_name));
        app.set_xalign(0.0);
        app.add_css_class("unixnotis-popup-header");

        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("unixnotis-popup-close");
        close.set_halign(Align::End);

        header.append(&app);
        header.append(&gtk::Box::new(gtk::Orientation::Horizontal, 1));
        header.append(&close);

        let summary = gtk::Label::new(Some(&notification.summary));
        summary.set_xalign(0.0);
        summary.set_wrap(true);
        summary.add_css_class("unixnotis-popup-summary");

        let body = gtk::Label::new(None);
        body.set_xalign(0.0);
        body.set_wrap(true);
        body.add_css_class("unixnotis-popup-body");
        set_label_markup(&body, &notification.body);

        root.append(&header);
        root.append(&summary);
        root.append(&body);

        if !notification.actions.is_empty() {
            let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            actions.add_css_class("unixnotis-popup-actions");
            for action in &notification.actions {
                let button = gtk::Button::with_label(&action.label);
                button.add_css_class("unixnotis-popup-action");
                let action_key = action.key.clone();
                let tx = self.command_tx.clone();
                let id = notification.id;
                button.connect_clicked(move |_| {
                    let _ = tx.send(UiCommand::InvokeAction {
                        id,
                        action_key: action_key.clone(),
                    });
                });
                actions.append(&button);
            }
            root.append(&actions);
        }

        let id = notification.id;
        let command_tx_close = self.command_tx.clone();
        close.connect_clicked(move |_| {
            let _ = command_tx_close.send(UiCommand::Dismiss(id));
        });

        let default_action = notification
            .actions
            .iter()
            .find(|action| action.key == "default")
            .map(|action| action.key.clone());
        if let Some(action_key) = default_action {
            let gesture = gtk::GestureClick::new();
            let tx = self.command_tx.clone();
            gesture.connect_released(move |_, _, _, _| {
                let _ = tx.send(UiCommand::InvokeAction {
                    id,
                    action_key: action_key.clone(),
                });
            });
            root.add_controller(gesture);
        }

        revealer.set_child(Some(&root));
        revealer.set_reveal_child(true);

        PopupEntry { revealer, root }
    }

    fn build_image_widget(&mut self, notification: &NotificationView) -> Option<gtk::Image> {
        let image = &notification.image;
        if let Some(texture) = image_data_texture(image) {
            let widget = gtk::Image::from_paintable(Some(&texture));
            widget.set_pixel_size(20);
            return Some(widget);
        }

        if !image.image_path.is_empty() {
            let path = image.image_path.as_str();
            if let Some(file_path) = file_path_from_hint(path) {
                // Decoded file:// paths allow loading icon files with escaped characters.
                if file_path.is_file() {
                    return Some(self.spawn_file_icon(file_path));
                }
            }
            return resolve_icon_image(path, 20);
        }

        let cache_key = format!("{}|{}", notification.app_name, notification.image.icon_name);
        if let Some(cached) = self.icon_cache.get(&cache_key) {
            return cached
                .as_ref()
                .and_then(|icon_name| resolve_icon_image(icon_name, 20));
        }

        let candidates = collect_icon_candidates(notification);
        let mut resolved = None;

        for candidate in &candidates {
            if let Some(icon_names) = self.desktop_icons.icons_for(candidate) {
                for icon_name in icon_names {
                    if resolve_icon_image(icon_name.as_str(), 20).is_some() {
                        resolved = Some(icon_name.clone());
                        break;
                    }
                }
                if resolved.is_some() {
                    break;
                }
            }
        }

        if resolved.is_none() {
            for candidate in &candidates {
                if resolve_icon_image(candidate, 20).is_some() {
                    resolved = Some(candidate.clone());
                    break;
                }
            }
        }

        self.cache_icon(cache_key, resolved.clone());
        resolved.and_then(|icon_name| resolve_icon_image(&icon_name, 20))
    }

    fn cache_icon(&mut self, cache_key: String, resolved: Option<String>) {
        // Bound the icon cache to avoid unbounded growth in long-running sessions.
        match self.icon_cache.entry(cache_key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.insert(resolved);
                return;
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let key = entry.key().clone();
                entry.insert(resolved);
                self.icon_cache_order.push_back(key);
            }
        }
        while self.icon_cache_order.len() > ICON_CACHE_MAX_ENTRIES {
            if let Some(evicted) = self.icon_cache_order.pop_front() {
                self.icon_cache.remove(&evicted);
            }
        }
    }

    fn spawn_file_icon(&self, path: PathBuf) -> gtk::Image {
        let widget = gtk::Image::new();
        let (tx, rx) = async_channel::bounded::<Result<RasterIcon, String>>(1);
        let widget_clone = widget.clone();
        // Apply the texture on the main loop to avoid GTK thread violations.
        glib::MainContext::default().spawn_local(async move {
            if let Ok(result) = rx.recv().await {
                match result {
                    Ok(icon) => {
                        let bytes = glib::Bytes::from(&icon.bytes);
                        let texture = gdk::MemoryTexture::new(
                            icon.width,
                            icon.height,
                            gdk::MemoryFormat::R8g8b8a8,
                            &bytes,
                            icon.stride as usize,
                        );
                        widget_clone.set_paintable(Some(&texture));
                    }
                    Err(err) => {
                        debug!(?err, "popup icon decode failed");
                    }
                }
            }
        });

        // Decode on a background worker pool to avoid spawning unbounded threads.
        IconDecodePool::global().submit(path, tx);

        widget
    }
}

fn set_label_markup(label: &gtk::Label, body: &str) {
    if body.is_empty() {
        label.set_text("");
        return;
    }
    label.set_markup(body);
}
