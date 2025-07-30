// Jackson Coxson
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::collections::HashMap;

use egui::{Color32, ComboBox, RichText, TextEdit};
use log::error;
use rfd::FileDialog;
use tokio::sync::mpsc::unbounded_channel;

use idevice::{
    IdeviceError, IdeviceService,
    diagnostics_relay::DiagnosticsRelayClient,
    lockdown::LockdownClient,
    usbmuxd::{UsbmuxdAddr, UsbmuxdConnection, UsbmuxdDevice},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

fn main() {
    println!("Startup");
    egui_logger::builder().init().unwrap();
    let (gui_sender, gui_recv) = unbounded_channel();
    let (idevice_sender, mut idevice_receiver) = unbounded_channel();
    idevice_sender.send(IdeviceCommands::GetDevices).unwrap();

    let mut supported_apps = HashMap::new();
    supported_apps.insert(
        "SideStore".to_string(),
        "ALTPairingFile.mobiledevicepairing".to_string(),
    );
    supported_apps.insert("Feather".to_string(), "pairingFile.plist".to_string());
    supported_apps.insert("StikDebug".to_string(), "pairingFile.plist".to_string());
    supported_apps.insert("Protokolle".to_string(), "pairingFile.plist".to_string());
    supported_apps.insert("Antrag".to_string(), "pairingFile.plist".to_string());

    let app = MyApp {
        devices: None,
        devices_placeholder: "Loading...".to_string(),
        selected_device: "".to_string(),
        device_info: None,
        gui_recv,
        idevice_sender: idevice_sender.clone(),
        show_logs: false,
        current_ioregistry: None,
        save_error: None,
        plane: "".to_string(),
        entry: "".to_string(),
        class: "".to_string(),
    };

    let d = eframe::icon_data::from_png_bytes(include_bytes!("../icon.png"))
        .expect("The icon data must be valid");
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1500.0, 800.0]),
        ..Default::default()
    };
    options.viewport.icon = Some(std::sync::Arc::new(d));

    // rt must be kept in scope for channel lifetimes, so we define and then spawn.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.spawn(async move {
        let gui_sender = gui_sender.clone();
        while let Some(command) = idevice_receiver.recv().await {
            match command {
                IdeviceCommands::GetDevices => {
                    // Connect to usbmuxd
                    let mut uc = match UsbmuxdConnection::default().await {
                        Ok(u) => u,
                        Err(e) => {
                            gui_sender.send(GuiCommands::NoUsbmuxd(e)).unwrap();
                            continue;
                        }
                    };

                    match uc.get_devices().await {
                        Ok(devs) => {
                            // We have to manually iterate to use async
                            let mut selections = HashMap::new();
                            for dev in devs {
                                let p = dev.to_provider(UsbmuxdAddr::default(), "idevice_pair");
                                let mut lc = match LockdownClient::connect(&p).await {
                                    Ok(l) => l,
                                    Err(e) => {
                                        error!("Failed to connect to lockdown: {e:?}");
                                        continue;
                                    }
                                };
                                let values = match lc.get_all_values(None).await {
                                    Ok(v) => v,
                                    Err(e) => {
                                        error!("Failed to get lockdown values: {e:?}");
                                        continue;
                                    }
                                };

                                // Get device name for selection
                                let device_name = match values.get("DeviceName") {
                                    Some(plist::Value::String(n)) => n.clone(),
                                    _ => {
                                        continue;
                                    }
                                };
                                selections.insert(device_name, dev);
                            }

                            gui_sender.send(GuiCommands::Devices(selections)).unwrap();
                        }
                        Err(e) => {
                            gui_sender.send(GuiCommands::GetDevicesFailure(e)).unwrap();
                        }
                    }
                }
                IdeviceCommands::IORegistsry((dev, plane, entry, class)) => {
                    let p = dev.to_provider(UsbmuxdAddr::default(), "ioreg_explorer");
                    let mut dc = match DiagnosticsRelayClient::connect(&p).await {
                        Ok(l) => l,
                        Err(e) => {
                            error!("Failed to connect to diagnostics relay: {e:?}");
                            continue;
                        }
                    };

                    let res = match dc.ioregistry(plane, entry, class).await {
                        Ok(l) => l,
                        Err(e) => {
                            error!("Failed to get IO registry: {e:?}");
                            continue;
                        }
                    };

                    gui_sender.send(GuiCommands::IORegistry(res)).unwrap();
                }
                IdeviceCommands::GetDeviceInfo(dev) => {
                    let p = dev.to_provider(UsbmuxdAddr::default(), "idevice_pair");
                    let mut lc = match LockdownClient::connect(&p).await {
                        Ok(l) => l,
                        Err(e) => {
                            error!("Failed to connect to lockdown: {e:?}");
                            continue;
                        }
                    };

                    let values = match lc.get_all_values(None).await {
                        Ok(v) => v,
                        Err(e) => {
                            error!("Failed to get lockdown values: {e:?}");
                            continue;
                        }
                    };

                    let mut device_info = Vec::with_capacity(5);

                    // Fixed order of fields in reverse order
                    let fields = [
                        ("Device Name", "DeviceName"),
                        ("Model", "ProductType"),
                        ("iOS Version", "ProductVersion"),
                        ("Build Number", "BuildVersion"),
                        ("UDID", "UniqueDeviceID"),
                    ];

                    for (display_name, key) in fields.iter() {
                        if let Some(plist::Value::String(value)) = values.get(key) {
                            device_info.push((display_name.to_string(), value.clone()));
                        }
                    }

                    gui_sender
                        .send(GuiCommands::DeviceInfo(device_info))
                        .unwrap();
                }
            };
        }
        eprintln!("Exited idevice loop!!");
    });

    eframe::run_native(
        "IORegistry Explorer",
        options,
        Box::new(|_| Ok(Box::new(app))),
    )
    .unwrap();
}

enum GuiCommands {
    NoUsbmuxd(IdeviceError),
    GetDevicesFailure(IdeviceError),
    Devices(HashMap<String, UsbmuxdDevice>),
    DeviceInfo(Vec<(String, String)>),
    IORegistry(Option<plist::Dictionary>),
}

enum IdeviceCommands {
    GetDevices,
    GetDeviceInfo(UsbmuxdDevice),
    IORegistsry(
        (
            UsbmuxdDevice,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    ),
}

struct MyApp {
    // Selector
    devices: Option<HashMap<String, UsbmuxdDevice>>,
    devices_placeholder: String,
    selected_device: String,
    device_info: Option<Vec<(String, String)>>,

    current_ioregistry: Option<plist::Dictionary>,
    save_error: Option<String>,

    // Inputs
    plane: String,
    entry: String,
    class: String,

    // Channel
    gui_recv: UnboundedReceiver<GuiCommands>,
    idevice_sender: UnboundedSender<IdeviceCommands>,

    show_logs: bool,
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Get updates from the idevice thread
        match self.gui_recv.try_recv() {
            Ok(msg) => match msg {
                GuiCommands::NoUsbmuxd(idevice_error) => {
                    let install_msg = if cfg!(windows) {
                        "Make sure you have iTunes installed from Apple's website, and that it's running."
                    } else if cfg!(target_os = "macos") {
                        "usbmuxd should be running by default on MacOS. Please raise an issue on GitHub."
                    } else {
                        "Make sure usbmuxd is installed and running."
                    };

                    self.devices_placeholder = format!(
                        "Failed to connect to usbmuxd! {install_msg}\n\n{idevice_error:#?}"
                    );
                }
                GuiCommands::Devices(vec) => self.devices = Some(vec),
                GuiCommands::DeviceInfo(info) => self.device_info = Some(info),
                GuiCommands::GetDevicesFailure(idevice_error) => {
                    self.devices_placeholder = format!(
                        "Failed to get list of connected devices from usbmuxd! {idevice_error:?}"
                    );
                }
                GuiCommands::IORegistry(i) => self.current_ioregistry = i,
            },
            Err(e) => match e {
                tokio::sync::mpsc::error::TryRecvError::Empty => {}
                tokio::sync::mpsc::error::TryRecvError::Disconnected => {
                    panic!("idevice crashed");
                }
            },
        }
        if self.show_logs {
            egui::Window::new("logs")
                .open(&mut self.show_logs)
                .show(ctx, |ui| {
                    egui_logger::logger_ui()
                        .warn_color(Color32::BLACK) // the yellow is too bright in dark mode
                        .log_levels([true, true, true, true, false])
                        .enable_category("idevice".to_string(), true)
                        // there should be a way to set default false...
                        .enable_category("mdns::mdns".to_string(), false)
                        .enable_category("eframe".to_string(), false)
                        .enable_category("eframe::native::glow_integration".to_string(), false)
                        .enable_category("egui_glow::shader_version".to_string(), false)
                        .enable_category("egui_glow::vao".to_string(), false)
                        .enable_category("egui_glow::painter".to_string(), false)
                        .enable_category("rustls::client::hs".to_string(), false)
                        .enable_category("rustls::client::tls12".to_string(), false)
                        .enable_category("rustls::client::common".to_string(), false)
                        .enable_category("idevice_pair::discover".to_string(), false)
                        .enable_category("reqwest::connect".to_string(), false)
                        .show(ui);
                });
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("IORegistry Explorer");
                    ui.separator();
                    let p_background_color = match ctx.theme() {
                        egui::Theme::Dark => Color32::BLACK,
                        egui::Theme::Light => Color32::LIGHT_GRAY,
                    };
                    egui::frame::Frame::new()
                        .corner_radius(3)
                        .inner_margin(3)
                        .fill(p_background_color)
                        .show(ui, |ui| {
                            ui.toggle_value(&mut self.show_logs, "logs");
                        });
                });
                match &self.devices {
                    Some(devs) => {
                        if devs.is_empty() {
                            ui.label("No devices connected! Plug one in via USB.");
                        } else {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label("Choose a device");
                                    ComboBox::from_label("")
                                        .selected_text(&self.selected_device)
                                        .show_ui(ui, |ui| {
                                            for (dev_name, dev) in devs {
                                                if ui
                                                    .selectable_value(
                                                        &mut self.selected_device,
                                                        dev_name.clone(),
                                                        dev_name.clone(),
                                                    )
                                                    .clicked()
                                                {
                                                    // Get device info immediately
                                                    self.device_info = None;

                                                    // Send all device info requests
                                                    let dev_clone = dev.clone();
                                                    self.idevice_sender
                                                        .send(IdeviceCommands::GetDeviceInfo(
                                                            dev_clone,
                                                        ))
                                                        .unwrap();
                                                    self.device_info = None;
                                                };
                                            }
                                        });
                                });

                                ui.separator();

                                // Show device info to the right if available
                                if let Some(info) = &self.device_info {
                                    ui.vertical(|ui| {
                                        for (key, value) in info {
                                            ui.horizontal(|ui| {
                                                ui.label(format!("{key}:"));
                                                ui.label(value);
                                            });
                                        }
                                    });
                                }
                            });
                        }
                        if ui.button("Refresh...").clicked() {
                            self.idevice_sender
                                .send(IdeviceCommands::GetDevices)
                                .unwrap();
                        }
                    }
                    None => {
                        ui.label(&self.devices_placeholder);
                    }
                }

                ui.separator();

                if let Some(dev) = self
                    .devices
                    .as_ref()
                    .and_then(|x| x.get(&self.selected_device))
                {
                    // How to load a file
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.heading("Plane");
                            ui.label("Entry Plane");
                            let response = ui.add(TextEdit::singleline(&mut self.plane));
                            if response.changed() {
                                self.idevice_sender
                                    .send(IdeviceCommands::IORegistsry((
                                        dev.clone(),
                                        if self.plane.is_empty() {
                                            None
                                        } else {
                                            Some(self.plane.clone())
                                        },
                                        if self.entry.is_empty() {
                                            None
                                        } else {
                                            Some(self.entry.clone())
                                        },
                                        if self.class.is_empty() {
                                            None
                                        } else {
                                            Some(self.class.clone())
                                        },
                                    )))
                                    .unwrap();
                            }
                        });
                        ui.separator();
                        ui.vertical(|ui| {
                            ui.heading("Name");
                            ui.label("Entry Name");
                            let response = ui.add(TextEdit::singleline(&mut self.entry));
                            if response.changed() {
                                self.idevice_sender
                                    .send(IdeviceCommands::IORegistsry((
                                        dev.clone(),
                                        if self.plane.is_empty() {
                                            None
                                        } else {
                                            Some(self.plane.clone())
                                        },
                                        if self.entry.is_empty() {
                                            None
                                        } else {
                                            Some(self.entry.clone())
                                        },
                                        if self.class.is_empty() {
                                            None
                                        } else {
                                            Some(self.class.clone())
                                        },
                                    )))
                                    .unwrap();
                            }
                        });
                        ui.separator();
                        ui.vertical(|ui| {
                            ui.heading("Class");
                            ui.label("Entry Class");
                            let response = ui.add(TextEdit::singleline(&mut self.class));
                            if response.changed() {
                                self.idevice_sender
                                    .send(IdeviceCommands::IORegistsry((
                                        dev.clone(),
                                        if self.plane.is_empty() {
                                            None
                                        } else {
                                            Some(self.plane.clone())
                                        },
                                        if self.entry.is_empty() {
                                            None
                                        } else {
                                            Some(self.entry.clone())
                                        },
                                        if self.class.is_empty() {
                                            None
                                        } else {
                                            Some(self.class.clone())
                                        },
                                    )))
                                    .unwrap();
                            }
                        });

                        ui.separator();
                        ui.vertical(|ui| {
                            ui.heading("Save to File");
                            if let Some(msg) = &self.save_error {
                                ui.label(RichText::new(msg).color(Color32::RED));
                            }
                            if ui.button("Save to File").clicked() {
                                if let Some(p) = FileDialog::new()
                                    .set_can_create_directories(true)
                                    .set_title("Save Pairing File")
                                    .set_file_name("ioreg.plist")
                                    .save_file()
                                {
                                    self.save_error = None;
                                    if let Err(e) = std::fs::write(
                                        p,
                                        idevice::pretty_print_dictionary(
                                            &self.current_ioregistry.clone().unwrap(),
                                        ),
                                    ) {
                                        self.save_error = Some(e.to_string());
                                    }
                                }
                            }
                        });
                    });

                    ui.separator();

                    if let Some(ioreg) = &self.current_ioregistry {
                        egui::Grid::new("reee").min_col_width(200.0).show(ui, |ui| {
                            let p_background_color = match ctx.theme() {
                                egui::Theme::Dark => Color32::BLACK,
                                egui::Theme::Light => Color32::LIGHT_GRAY,
                            };
                            egui::frame::Frame::new()
                                .corner_radius(10)
                                .inner_margin(10)
                                .fill(p_background_color)
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(idevice::pretty_print_dictionary(ioreg))
                                            .monospace(),
                                    );
                                });
                        });
                    }
                }
            });
        });
    }
}
