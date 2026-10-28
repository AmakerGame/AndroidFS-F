//! Main window: device list, controls, settings and log.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use log::{error, info, LevelFilter};
use widestring::U16CString;
use winapi::um::winuser::{MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST};

use crate::daemon_cmd::AccessMode;
use crate::service::{self, Command, DeviceEntry, DeviceState, SharedState};
use crate::settings::{self, DeviceSetting, Settings};

const ACCENT: egui::Color32 = egui::Color32::from_rgb(61, 220, 132);
const WARN: egui::Color32 = egui::Color32::from_rgb(240, 180, 60);
const DANGER: egui::Color32 = egui::Color32::from_rgb(235, 87, 87);
const MUTED: egui::Color32 = egui::Color32::from_rgb(140, 150, 165);
const SINGLE_INSTANCE_PORT: u16 = 47831;
const REPO_URL: &str = "https://github.com/AmakerGame/AndroidFS-F";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Devices,
    Settings,
    Log,
    About,
}

struct App {
    shared: SharedState,
    tab: Tab,
    form: Settings,
    autostart: bool,
    note: Option<(bool, String)>,
    minimize_on_start: bool,
    connect_text: String,
}

impl App {
    fn new(shared: SharedState, minimize_on_start: bool) -> Self {
        let form = service::lock(&shared).settings.clone();
        App {
            shared,
            tab: Tab::Devices,
            form,
            autostart: settings::autostart_enabled(),
            note: None,
            minimize_on_start,
            connect_text: String::new(),
        }
    }

    fn send(&self, command: Command) {
        service::lock(&self.shared).commands.push_back(command);
    }

    fn applied(&self) -> Settings {
        service::lock(&self.shared).settings.clone()
    }

    fn apply_form(&mut self) {
        if !self.form.root.trim().is_empty() && !crate::daemon_cmd::is_valid_root_setting(&self.form.root) {
            self.note = Some((
                false,
                "Root directory must be an absolute Android path (letters, digits, spaces and / _ . - + @ ( ) , = ~)"
                    .to_string(),
            ));
            return;
        }
        service::lock(&self.shared).settings = self.form.clone();
        self.note = match settings::save(&self.form) {
            Ok(()) => Some((true, "Settings saved.".to_string())),
            Err(err) => Some((false, format!("Could not save settings: {err}"))),
        };
    }

    /// Saves per-device overrides immediately (they apply at the next mount).
    fn save_device_settings(&mut self) {
        let mut applied = self.applied();
        applied.per_device = self.form.per_device.clone();
        service::lock(&self.shared).settings = applied.clone();
        self.note = match settings::save(&applied) {
            Ok(()) => None,
            Err(err) => Some((false, format!("Could not save settings: {err}"))),
        };
    }

    fn device_access_ui(&mut self, ui: &mut egui::Ui, serial: &str) {
        let mut entry = self.form.per_device.get(serial).cloned().unwrap_or_default();
        let before = entry.clone();
        let default_label = match self.form.access_mode {
            AccessMode::Auto => "Default (automatic)",
            AccessMode::Shell => "Default (without root)",
            AccessMode::Root => "Default (root)",
        };
        let current = match entry.access_mode {
            None => default_label,
            Some(AccessMode::Auto) => "Automatic",
            Some(AccessMode::Shell) => "Without root",
            Some(AccessMode::Root) => "Root (su)",
        };
        ui.collapsing("Access for this device", |ui| {
            if ui.small_button("Open adb shell").clicked() {
                let adb = service::lock(&self.shared).adb_path_in_use.clone();
                open_adb_shell(&adb, serial);
            }
            let letter_text = entry.letter.clone().unwrap_or_else(|| "Automatic".to_string());
            egui::ComboBox::from_id_source(format!("letter_{serial}"))
                .selected_text(format!("Drive letter: {letter_text}"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut entry.letter, None, "Automatic");
                    for letter in service::MOUNT_POINTS {
                        ui.selectable_value(&mut entry.letter, Some(letter.to_string()), *letter);
                    }
                });
            egui::ComboBox::from_id_source(format!("access_{serial}"))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut entry.access_mode, None, default_label);
                    ui.selectable_value(&mut entry.access_mode, Some(AccessMode::Auto), "Automatic");
                    ui.selectable_value(&mut entry.access_mode, Some(AccessMode::Shell), "Without root");
                    ui.selectable_value(&mut entry.access_mode, Some(AccessMode::Root), "Root (su)");
                });
            let mut read_only = entry.read_only.unwrap_or(self.form.read_only);
            if ui
                .checkbox(&mut read_only, "Read-only (Windows cannot change this device)")
                .changed()
            {
                entry.read_only = Some(read_only);
            }
            let mut root = entry.root.clone().unwrap_or_default();
            ui.add(
                egui::TextEdit::singleline(&mut root)
                    .hint_text("Folder (empty = default)")
                    .desired_width(220.0),
            );
            ui.horizontal(|ui| {
                if ui.small_button("Default").clicked() {
                    root.clear();
                    entry.root = None;
                }
                if ui.small_button("Storage").clicked() {
                    root = "/storage/emulated/0".to_string();
                }
                if ui.small_button("SD / USB").clicked() {
                    root = "@external".to_string();
                }
                if ui.small_button("/").clicked() {
                    root = "/".to_string();
                }
                if ui.small_button("/data").clicked() {
                    root = "/data".to_string();
                }
            });
            if !root.is_empty() || entry.root.is_some() {
                entry.root = if root.is_empty() { None } else { Some(root) };
            }
            ui.label(
                egui::RichText::new("Unmount and Mount again to apply. With root and no folder, the whole device (/) is shown.")
                    .small()
                    .color(MUTED),
            );
        });
        let valid = entry.root.as_deref().map_or(true, |r| r.trim().is_empty() || crate::daemon_cmd::is_valid_root_setting(r));
        if entry != before && valid {
            if entry == DeviceSetting::default() {
                self.form.per_device.remove(serial);
            } else {
                self.form.per_device.insert(serial.to_string(), entry);
            }
            self.save_device_settings();
        }
    }

    fn devices_tab(&mut self, ui: &mut egui::Ui, devices: &[DeviceEntry], adb_error: &Option<String>) {
        ui.horizontal(|ui| {
            if ui.button("Refresh").clicked() {
                self.send(Command::Refresh);
            }
            let mut auto = self.form.auto_mount;
            if ui.checkbox(&mut auto, "Mount devices automatically").changed() {
                self.form.auto_mount = auto;
                let mut applied = self.applied();
                applied.auto_mount = auto;
                service::lock(&self.shared).settings = applied.clone();
                let _ = settings::save(&applied);
            }
        });
        ui.add_space(6.0);

        if let Some(message) = adb_error {
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(70, 30, 30))
                .rounding(6.0)
                .inner_margin(egui::Margin::same(10.0))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("ADB is not available").color(DANGER).strong());
                    ui.label(message.as_str());
                    ui.label(
                        egui::RichText::new(
                            "Install Android platform-tools and add adb.exe to PATH, or set the path in Settings.",
                        )
                        .color(MUTED),
                    );
                });
            ui.add_space(6.0);
        }

        let applied_now = self.applied();
        let hosts = applied_now.hosts.clone();
        self.form.hosts = hosts.clone();
        self.form.per_device = applied_now.per_device.clone();
        ui.horizontal(|ui| {
            ui.label("Network device:");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.connect_text)
                    .hint_text("192.168.0.50 or 192.168.0.50:5555")
                    .desired_width(230.0),
            );
            let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if (ui.button("Connect").clicked() || enter) && !self.connect_text.trim().is_empty() {
                self.send(Command::Connect(self.connect_text.trim().to_string()));
                self.connect_text.clear();
            }
        });
        for host in &hosts {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("Saved: {host}")).small().color(MUTED));
                if ui.small_button("Forget").clicked() {
                    self.send(Command::Forget(host.clone()));
                }
            });
        }
        let problems = service::lock(&self.shared).problems.clone();
        for (serial, state) in &problems {
            let hint = match state.as_str() {
                "unauthorized" => "Unlock the phone and tap Allow on the USB debugging prompt (tick Always allow).",
                "authorizing" => "Waiting for the USB debugging prompt on the phone.",
                _ => "Offline: re-plug the cable, toggle USB debugging, or run adb kill-server.",
            };
            ui.label(
                egui::RichText::new(format!("{serial} - {state}. {hint}"))
                    .small()
                    .color(WARN),
            );
        }
        ui.add_space(6.0);

        if devices.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("No devices detected").size(20.0).strong());
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "Connect your phone with a USB cable, enable USB debugging\nand allow this computer on the phone screen.",
                    )
                    .color(MUTED),
                );
            });
            return;
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("devices_grid")
                .num_columns(4)
                .spacing([24.0, 12.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Device").color(MUTED));
                    ui.label(egui::RichText::new("CPU").color(MUTED));
                    ui.label(egui::RichText::new("Status").color(MUTED));
                    ui.label(egui::RichText::new("Actions").color(MUTED));
                    ui.end_row();

                    for device in devices {
                        ui.vertical(|ui| {
                            let name = if device.model.is_empty() {
                                "Android device"
                            } else {
                                device.model.as_str()
                            };
                            ui.label(egui::RichText::new(name).strong());
                            ui.label(egui::RichText::new(device.serial.as_str()).monospace().color(MUTED));
                            if let Some(info) = &device.info {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} - {}{}",
                                        info.access,
                                        info.root_dir,
                                        if info.read_only { " (read-only)" } else { "" }
                                    ))
                                        .small()
                                        .color(if info.entries.is_none() { WARN } else { MUTED }),
                                );
                                match info.entries {
                                    Some(0) => {
                                        ui.label(
                                            egui::RichText::new("The folder is empty on the phone")
                                                .small()
                                                .color(WARN),
                                        );
                                    }
                                    None => {
                                        ui.label(
                                            egui::RichText::new(
                                                "Folder not readable - set access mode to Root in Settings",
                                            )
                                            .small()
                                            .color(WARN),
                                        );
                                    }
                                    Some(_) => {}
                                }
                                if let Some(note) = &info.note {
                                    ui.label(egui::RichText::new(note.as_str()).small().color(WARN));
                                }
                            }
                            self.device_access_ui(ui, &device.serial);
                        });
                        ui.label(if device.abi.is_empty() { "-" } else { device.abi.as_str() });
                        state_label(ui, &device.state);
                        ui.horizontal(|ui| match &device.state {
                            DeviceState::Mounted(letter) => {
                                if ui.button("Open").clicked() {
                                    open_in_shell(&format!("{letter}\\"));
                                }
                                if ui.button("Unmount").clicked() {
                                    self.send(Command::Unmount(device.serial.clone()));
                                }
                            }
                            DeviceState::Mounting | DeviceState::Unmounting => {
                                ui.spinner();
                            }
                            DeviceState::Detected | DeviceState::Failed(_) => {
                                if ui.button("Mount").clicked() {
                                    self.send(Command::Mount(device.serial.clone()));
                                }
                            }
                        });
                        ui.end_row();
                    }
                });
        });
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui, adb_in_use: &str) {
        let applied = self.applied();
        self.form.hosts = applied.hosts.clone();
        self.form.per_device = applied.per_device.clone();
        ui.heading("Settings");
        ui.add_space(8.0);

        ui.group(|ui| {
            ui.label(egui::RichText::new("General").strong());
            ui.checkbox(&mut self.form.auto_mount, "Mount devices automatically when connected");
            ui.checkbox(
                &mut self.form.read_only,
                "Mount read-only by default (protects the phone from accidental changes)",
            );
            let mut autostart = self.autostart;
            if ui.checkbox(&mut autostart, "Start with Windows (minimized)").changed() {
                match settings::set_autostart(autostart) {
                    Ok(()) => {
                        self.autostart = autostart;
                        self.note = None;
                    }
                    Err(err) => {
                        self.note = Some((false, format!("Could not change autostart: {err}")));
                    }
                }
            }
        });
        ui.add_space(8.0);

        ui.group(|ui| {
            ui.label(egui::RichText::new("Android Debug Bridge").strong());
            ui.label("Path to adb.exe (leave empty to auto-detect):");
            ui.add(
                egui::TextEdit::singleline(&mut self.form.adb_path)
                    .hint_text("Auto-detect")
                    .desired_width(f32::INFINITY),
            );
            ui.label(egui::RichText::new(format!("In use: {adb_in_use}")).color(MUTED));
        });
        ui.add_space(8.0);

        ui.group(|ui| {
            ui.label(egui::RichText::new("Access on the phone").strong());
            ui.radio_value(
                &mut self.form.access_mode,
                AccessMode::Auto,
                "Automatic - without root, switch to root only if the folder cannot be read",
            );
            ui.radio_value(
                &mut self.form.access_mode,
                AccessMode::Shell,
                "Without root - works on every phone, limited to what the ADB shell may read",
            );
            ui.radio_value(
                &mut self.form.access_mode,
                AccessMode::Root,
                "Root (su) - full access on rooted phones; approve the request on the phone",
            );
            ui.label(
                egui::RichText::new("Applies the next time a device is mounted (Unmount, then Mount).")
                    .color(MUTED),
            );
        });
        ui.add_space(8.0);

        ui.group(|ui| {
            ui.label(egui::RichText::new("Drive root on the phone").strong());
            ui.label("Android directory shown as the drive (leave empty for shared storage):");
            ui.add(
                egui::TextEdit::singleline(&mut self.form.root)
                    .hint_text("Shared storage (auto-detected)")
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                if ui.small_button("Shared storage").clicked() {
                    self.form.root.clear();
                }
                if ui.small_button("Whole device (/)").clicked() {
                    self.form.root = "/".to_string();
                }
                if ui.small_button("SD / USB drive").clicked() {
                    self.form.root = "@external".to_string();
                }
                if ui.small_button("All storages (/storage)").clicked() {
                    self.form.root = "/storage".to_string();
                }
                if ui.small_button("App data (/data)").clicked() {
                    self.form.root = "/data".to_string();
                }
            });
            ui.label(
                egui::RichText::new(
                    "/data and most of / need root. Spaces in the path are allowed.",
                )
                .color(MUTED),
            );
        });
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            let dirty = self.form != applied;
            if ui.add_enabled(dirty, egui::Button::new("Apply")).clicked() {
                self.apply_form();
            }
            if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                self.form = applied.clone();
                self.note = None;
            }
        });
        if let Some((ok, text)) = &self.note {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(text.as_str()).color(if *ok { ACCENT } else { DANGER }));
        }
    }

    fn log_tab(&mut self, ui: &mut egui::Ui) {
        let lines: Vec<String> = service::lock(&self.shared).log.iter().cloned().collect();
        ui.horizontal(|ui| {
            if ui.button("Copy").clicked() {
                let text = lines.join("\n");
                ui.output_mut(|out| out.copied_text = text);
            }
            if ui.button("Copy diagnostics").clicked() {
                let guard = service::lock(&self.shared);
                let mut text = format!(
                    "AndroidFS-F {}\nadb: {}\nsettings: {:?}\n",
                    env!("CARGO_PKG_VERSION"),
                    guard.adb_path_in_use,
                    guard.settings
                );
                for device in &guard.devices {
                    text.push_str(&format!("device {}: {:?} abi={} info={:?}\n", device.serial, device.state, device.abi, device.info));
                }
                text.push_str(&format!("problems: {:?}\n--- log ---\n{}", guard.problems, lines.join("\n")));
                drop(guard);
                ui.output_mut(|out| out.copied_text = text);
            }
            if ui.button("Clear").clicked() {
                service::lock(&self.shared).log.clear();
            }
            if ui.button("Open log folder").clicked() {
                open_in_shell(&settings::data_dir().to_string_lossy());
            }
        });
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for line in &lines {
                    let color = if line.contains("ERROR") { DANGER } else { MUTED };
                    ui.label(egui::RichText::new(line.as_str()).monospace().color(color));
                }
            });
    }

    fn about_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("AndroidFS-F");
        ui.label(egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(MUTED));
        ui.add_space(8.0);
        ui.label("Mounts an Android device as a Windows drive over ADB and Dokan, without MTP.");
        ui.add_space(8.0);
        ui.label("Fork of AndroidFS by Lauriethefish. Licensed under GPL-3.0.");
        ui.add_space(8.0);
        ui.label("Supported devices: arm64-v8a and armeabi-v7a, USB debugging enabled. Works with and without root.");
        ui.add_space(12.0);
        if ui.button("Open project page").clicked() {
            open_in_shell(REPO_URL);
        }
    }
}

fn state_label(ui: &mut egui::Ui, state: &DeviceState) {
    match state {
        DeviceState::Detected => {
            ui.label(egui::RichText::new("Ready").color(MUTED));
        }
        DeviceState::Mounting => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("Mounting...").color(WARN));
            });
        }
        DeviceState::Unmounting => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("Unmounting...").color(WARN));
            });
        }
        DeviceState::Mounted(letter) => {
            ui.label(egui::RichText::new(format!("Mounted as {letter}\\")).color(ACCENT).strong());
        }
        DeviceState::Failed(message) => {
            ui.label(egui::RichText::new("Failed").color(DANGER).strong())
                .on_hover_text(message.clone());
        }
    }
}

/// Opens a console with `adb -s <serial> shell` for this device.
fn open_adb_shell(adb: &str, serial: &str) {
    use std::os::windows::process::CommandExt;
    let plain = |text: &str| text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-' | '_'));
    if !plain(serial) || adb.contains('"') {
        return;
    }
    let _ = std::process::Command::new("cmd")
        .raw_arg(format!("/C start \"adb {serial}\" cmd /K \"\"{adb}\" -s {serial} shell\""))
        .spawn();
}

fn open_in_shell(target: &str) {
    let _ = std::process::Command::new("explorer.exe").arg(target).spawn();
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(500));

        if self.minimize_on_start {
            self.minimize_on_start = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        let (devices, adb_error, adb_in_use) = {
            let guard = service::lock(&self.shared);
            (
                guard.devices.clone(),
                guard.adb_error.clone(),
                guard.adb_path_in_use.clone(),
            )
        };

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("AndroidFS-F").size(20.0).strong().color(ACCENT));
                ui.add_space(16.0);
                ui.selectable_value(&mut self.tab, Tab::Devices, "Devices");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
                ui.selectable_value(&mut self.tab, Tab::Log, "Log");
                ui.selectable_value(&mut self.tab, Tab::About, "About");
            });
            ui.add_space(6.0);
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                let mounted = devices
                    .iter()
                    .filter(|d| matches!(d.state, DeviceState::Mounted(_)))
                    .count();
                ui.label(format!("{} device(s), {} mounted", devices.len(), mounted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if adb_error.is_some() {
                        ui.label(egui::RichText::new("ADB error").color(DANGER));
                    } else {
                        ui.label(egui::RichText::new("ADB ready").color(ACCENT));
                    }
                });
            });
            ui.add_space(3.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Devices => self.devices_tab(ui, &devices, &adb_error),
            Tab::Settings => self.settings_tab(ui, &adb_in_use),
            Tab::Log => self.log_tab(ui),
            Tab::About => self.about_tab(ui),
        });
    }
}

fn message_box(text: &str) {
    let (Ok(title), Ok(body)) = (U16CString::from_str("AndroidFS-F"), U16CString::from_str(text))
    else {
        return;
    };
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST,
        );
    }
}

fn init_logging() {
    let mut builder = env_logger::Builder::new();
    builder.parse_default_env();
    if std::env::var_os("RUST_LOG").is_none() {
        builder.filter_level(LevelFilter::Info);
    }
    let _ = std::fs::create_dir_all(settings::data_dir());
    // Keep the log from growing forever: roll it over at 2 MB.
    if std::fs::metadata(settings::log_path()).map(|m| m.len() > 2 * 1024 * 1024).unwrap_or(false) {
        let _ = std::fs::rename(settings::log_path(), settings::log_path().with_extension("log.old"));
    }
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(settings::log_path())
    {
        builder.target(env_logger::Target::Pipe(Box::new(file)));
    }
    let _ = builder.try_init();
}

fn load_icon() -> egui::IconData {
    egui::IconData {
        rgba: include_bytes!("../assets/icon64.rgba").to_vec(),
        width: 64,
        height: 64,
    }
}

pub fn run() {
    let background = std::env::args().any(|arg| arg == "--background" || arg == "--silent");
    init_logging();
    std::panic::set_hook(Box::new(|info| error!("panic: {info}")));

    // Only one window/instance at a time: a second start would fight over drive letters.
    let _instance = match std::net::TcpListener::bind(("127.0.0.1", SINGLE_INSTANCE_PORT)) {
        Ok(listener) => listener,
        Err(_) => {
            message_box("AndroidFS-F is already running. Look for its window in the taskbar.");
            return;
        }
    };

    info!("AndroidFS-F {} started", env!("CARGO_PKG_VERSION"));
    let shared: SharedState = Arc::new(Mutex::new(service::Shared::new(settings::load())));
    service::spawn_engine(shared.clone());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("AndroidFS-F")
            .with_inner_size([900.0, 580.0])
            .with_min_inner_size([700.0, 420.0])
            .with_icon(Arc::new(load_icon())),
        ..Default::default()
    };

    let app_shared = shared.clone();
    let result = eframe::run_native(
        "AndroidFS-F",
        options,
        Box::new(move |cc| {
            let mut style = (*cc.egui_ctx.style()).clone();
            style.spacing.item_spacing = egui::vec2(10.0, 8.0);
            style.spacing.button_padding = egui::vec2(12.0, 6.0);
            cc.egui_ctx.set_style(style);
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Box::new(App::new(app_shared, background))
        }),
    );
    if let Err(err) = result {
        error!("window error: {err}");
        message_box(&format!(
            "The window could not be created: {err}\n\nYour graphics driver may not support OpenGL 2.0."
        ));
    }

    service::shutdown(&shared);
    info!("AndroidFS-F exited");
}
