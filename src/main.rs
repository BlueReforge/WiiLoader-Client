#![windows_subsystem = "windows"]

use eframe::egui;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([580.0, 520.0])
            .with_resizable(true),
        ..Default::default()
    };

    eframe::run_native(
        "WiiLoader Client",
        options,
        Box::new(|_cc| Ok(Box::new(WiiloadApp::default()))),
    )
}

struct WiiloadApp {
    ip_input: String,
    file_path: String,
    logs: Vec<String>,
    is_transmitting: bool,
    use_compression: bool,
}

impl Default for WiiloadApp {
    fn default() -> Self {
        Self {
            ip_input: String::new(),
            file_path: String::new(),
            logs: vec!["[Init] Ready. Select a .dol / .elf / .zip and enter the Wii IP.".into()],
            is_transmitting: false,
            use_compression: true,
        }
    }
}

impl eframe::App for WiiloadApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("WiiLoader Network Deployer");
            ui.label("Sends .dol / .elf / .zip to Homebrew Channel (port 4299)");
            ui.separator();
            ui.add_space(8.0);

            ui.label("Wii IP Address:");
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.ip_input);
                if ui.button("Validate").clicked() {
                    match self.ip_input.trim().parse::<Ipv4Addr>() {
                        Ok(ip) => self.logs.push(format!("[Net] Valid IP: {}", ip)),
                        Err(_) => self.logs.push("[Error] Invalid IPv4 address".into()),
                    }
                }
            });

            ui.add_space(10.0);

            ui.label("Payload (.dol / .elf / .zip):");
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.file_path);
                if ui.button("Browse…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Wii Homebrew", &["dol", "elf", "zip"])
                        .pick_file()
                    {
                        self.file_path = path.display().to_string();
                        self.logs.push(format!("[File] Selected: {}", self.file_path));
                    }
                }
            });

            ui.add_space(8.0);
            ui.checkbox(&mut self.use_compression, "Use zlib compression (recommended)");

            ui.add_space(12.0);

            let ip_ok = self.ip_input.trim().parse::<Ipv4Addr>().is_ok();
            let file_ok = !self.file_path.is_empty() && Path::new(&self.file_path).exists();
            let can_send = ip_ok && file_ok && !self.is_transmitting;

            if ui
                .add_enabled(
                    can_send,
                    egui::Button::new("Push to Wii").min_size(egui::vec2(200.0, 36.0)),
                )
                .clicked()
            {
                self.is_transmitting = true;
                let ip = self.ip_input.clone();
                let path = self.file_path.clone();
                let compress = self.use_compression;

                self.logs
                    .push(format!("[Net] Connecting to {}:4299 …", ip.trim()));

                match send_to_wii(&ip, &path, compress, &mut self.logs) {
                    Ok(()) => self
                        .logs
                        .push("[Success] Transfer completed. Check HBC.".into()),
                    Err(e) => self.logs.push(format!("[Failure] {}", e)),
                }

                self.is_transmitting = false;
            }

            ui.add_space(12.0);
            ui.label("Log:");
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &self.logs {
                        ui.monospace(line);
                    }
                });
        });
    }
}

fn send_to_wii(
    ip: &str,
    file_path: &str,
    compress: bool,
    logs: &mut Vec<String>,
) -> io::Result<()> {
    let path = Path::new(file_path);
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("boot.dol")
        .to_string();

    let mut file = File::open(path)?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)?;
    let uncompressed_size = raw.len() as u32;
    logs.push(format!(
        "[File] Loaded {} bytes ({})",
        uncompressed_size, filename
    ));

    let (payload, compressed_size) = if compress {
        logs.push("[Protocol] Compressing with zlib (level 9)…".into());
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&raw)?;
        let compressed = encoder.finish()?;
        let csize = compressed.len() as u32;
        logs.push(format!(
            "[Protocol] Compressed: {} → {} bytes ({:.1}%)",
            uncompressed_size,
            csize,
            (csize as f64 / uncompressed_size as f64) * 100.0
        ));
        (compressed, csize)
    } else {
        (raw, uncompressed_size)
    };

    let addr: SocketAddr = format!("{}:4299", ip.trim()).parse().map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidInput, format!("Bad address: {}", e))
    })?;

    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    logs.push("[TCP] Connected.".into());

    let mut header = Vec::with_capacity(16);
    header.extend_from_slice(b"HAXX");
    header.push(0);
    header.push(5);
    header.extend_from_slice(&0u16.to_be_bytes());
    header.extend_from_slice(&compressed_size.to_be_bytes());
    header.extend_from_slice(&uncompressed_size.to_be_bytes());

    logs.push("[Protocol] Sending HAXX header…".into());
    stream.write_all(&header)?;

    logs.push(format!(
        "[Protocol] Sending {} bytes of data…",
        compressed_size
    ));
    stream.write_all(&payload)?;
    stream.flush()?;

    logs.push("[Protocol] Transfer finished.".into());

    let mut buf = [0u8; 4];
    let _ = stream.read(&mut buf);

    Ok(())
}