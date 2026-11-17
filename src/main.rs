#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use hidapi::HidApi;
use serde::{Deserialize, Serialize};
use std::{
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    Icon, TrayIconBuilder, TrayIconEvent,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyNameTextW, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD,
    KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
    MAPVK_VK_TO_VSC_EX, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, TranslateMessage, MSG,
};

// ---------- Constantes do controle ----------
const VID: u16 = 0x3537; // GameSir
const USAGE_PAGE: u16 = 0xFFF0; // canal vendor-defined usado pelo GameSir Connect
const BYTE_IDX: usize = 14; // byte que carrega L4/R4
const MASKS: [u8; 2] = [0x04, 0x08]; // L4, R4
const NAMES: [&str; 2] = ["L4", "R4"];
const REPEAT_DELAY: Duration = Duration::from_millis(500);
const REPEAT_RATE: Duration = Duration::from_millis(33);
const LOCK_ADDR: &str = "127.0.0.1:47213"; // trava de instância única

// Ctrl, Shift, Alt, Win (VKs genéricos)
const MODS: [u16; 4] = [0x11, 0x10, 0x12, 0x5B];

const GLYPHS: [[&str; 7]; 2] = [
    [
        "#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#..",
    ], // T
    [
        ".###.", "#...#", "....#", "..##.", "....#", "#...#", ".###.",
    ], // 3
];

fn make_icon() -> (Vec<u8>, u32, u32) {
    const SIZE: usize = 64;
    const SCALE: usize = 5;
    let bg = [40u8, 150, 70, 255]; // fundo verde
    let fg = [255u8, 255, 255, 255]; // letras brancas
    let mut rgba: Vec<u8> = (0..SIZE * SIZE).flat_map(|_| bg).collect();

    let text_w = (5 + 1 + 5) * SCALE;
    let text_h = 7 * SCALE;
    let (x0, y0) = ((SIZE - text_w) / 2, (SIZE - text_h) / 2);

    for (g, glyph) in GLYPHS.iter().enumerate() {
        for (row, line) in glyph.iter().enumerate() {
            for (col, ch) in line.chars().enumerate() {
                if ch != '#' {
                    continue;
                }
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        let x = x0 + (g * 6 + col) * SCALE + dx;
                        let y = y0 + row * SCALE + dy;
                        let i = (y * SIZE + x) * 4;
                        rgba[i..i + 4].copy_from_slice(&fg);
                    }
                }
            }
        }
    }
    (rgba, SIZE as u32, SIZE as u32)
}

// ---------- Configuração (config.json ao lado do .exe) ----------
#[derive(Serialize, Deserialize, Clone, Default)]
struct ButtonCfg {
    keys: Vec<u16>, // VKs a pressionar em ordem (ex.: [Ctrl, C])
    repeat: bool,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct Config {
    buttons: [ButtonCfg; 2],
}

fn config_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("config.json")))
        .unwrap_or_else(|| PathBuf::from("config.json"))
}

fn load_config() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(c: &Config) {
    let p = config_path();
    if let Ok(s) = serde_json::to_string_pretty(c) {
        let _ = std::fs::write(p, s);
    }
}

// ---------- Teclado (Win32) ----------
fn key_down(vk: u16) -> bool {
    unsafe { (GetAsyncKeyState(vk as i32) as u16) & 0x8000 != 0 }
}

fn is_modifier(vk: u16) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

fn send_key(vk: u16, up: bool) {
    unsafe {
        let sc = MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC_EX);
        let mut flags = KEYBD_EVENT_FLAGS(0);
        let (w_vk, w_scan) = if sc != 0 {
            flags = flags | KEYEVENTF_SCANCODE;
            if (sc >> 8) == 0xE0 {
                flags = flags | KEYEVENTF_EXTENDEDKEY;
            }
            (VIRTUAL_KEY(0), (sc & 0xFF) as u16)
        } else {
            (VIRTUAL_KEY(vk), 0)
        };
        if up {
            flags = flags | KEYEVENTF_KEYUP;
        }
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: w_vk,
                    wScan: w_scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

fn key_name(vk: u16) -> String {
    match vk {
        0x11 => return "Ctrl".into(),
        0x10 => return "Shift".into(),
        0x12 => return "Alt".into(),
        0x5B => return "Win".into(),
        _ => {}
    }
    unsafe {
        let sc = MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC_EX);
        let ext = (sc >> 8) == 0xE0;
        let lparam = (((sc & 0xFF) << 16) | if ext { 1 << 24 } else { 0 }) as i32;
        let mut buf = [0u16; 64];
        let n = GetKeyNameTextW(lparam, &mut buf);
        if n > 0 {
            return String::from_utf16_lossy(&buf[..n as usize]);
        }
    }
    format!("VK 0x{:02X}", vk)
}

fn describe(keys: &[u16]) -> String {
    keys.iter()
        .map(|&k| key_name(k))
        .collect::<Vec<_>>()
        .join(" + ")
}

// ---------- Leitura do controle ----------
#[derive(Clone, Copy, Default)]
struct Status {
    connected: bool,
    pressed: [bool; 2],
}

fn set_status(st: &Arc<Mutex<Status>>, ctx: &Option<egui::Context>, f: impl FnOnce(&mut Status)) {
    f(&mut *st.lock().unwrap());
    if let Some(c) = ctx {
        c.request_repaint();
    }
}

#[derive(Default)]
struct Btn {
    pressed: bool,
    held: Vec<u16>,
    next_repeat: Option<Instant>,
}

fn release(b: &mut Btn) {
    for &k in b.held.iter().rev() {
        send_key(k, true);
    }
    b.held.clear();
    b.next_repeat = None;
}

/// `send_keys = true`: processo de fundo (envia as teclas, relendo o config.json a cada aperto).
/// `send_keys = false`: janela (só atualiza os indicadores).
fn session(
    dev: &hidapi::HidDevice,
    status: &Arc<Mutex<Status>>,
    ctx: &Option<egui::Context>,
    send_keys: bool,
) {
    let mut buf = [0u8; 64];
    let mut btns: [Btn; 2] = Default::default();

    loop {
        // Timeout curto só enquanto há repetição pendente; senão dorme até chegar dado
        let repeating = btns.iter().any(|b| b.next_repeat.is_some());
        let timeout = if repeating { 10 } else { 200 };

        match dev.read_timeout(&mut buf, timeout) {
            Ok(n) if n > BYTE_IDX => {
                for i in 0..2 {
                    let p = (buf[BYTE_IDX] & MASKS[i]) != 0;
                    if p == btns[i].pressed {
                        continue;
                    }
                    btns[i].pressed = p;
                    if p {
                        if send_keys {
                            let c = load_config().buttons[i].clone();
                            if !c.keys.is_empty() {
                                for &k in &c.keys {
                                    send_key(k, false);
                                }
                                btns[i].held = c.keys;
                                if c.repeat {
                                    btns[i].next_repeat = Some(Instant::now() + REPEAT_DELAY);
                                }
                            }
                        }
                    } else {
                        release(&mut btns[i]);
                    }
                    set_status(status, ctx, |s| s.pressed[i] = p);
                }
            }
            Ok(_) => {}
            Err(_) => break, // controle desconectado
        }

        // Repetição automática (repete só a última tecla do atalho)
        let now = Instant::now();
        for b in btns.iter_mut() {
            if let (Some(t), Some(&k)) = (b.next_repeat, b.held.last()) {
                if now >= t {
                    send_key(k, false);
                    b.next_repeat = Some(now + REPEAT_RATE);
                }
            }
        }
    }

    for b in btns.iter_mut() {
        release(b);
    }
}

fn worker(status: Arc<Mutex<Status>>, ctx: Option<egui::Context>, send_keys: bool) {
    let mut api = loop {
        match HidApi::new() {
            Ok(a) => break a,
            Err(_) => thread::sleep(Duration::from_secs(2)),
        }
    };

    loop {
        let _ = api.refresh_devices();
        let path = api
            .device_list()
            .find(|d| d.vendor_id() == VID && d.usage_page() == USAGE_PAGE)
            .map(|d| d.path().to_owned());
        let dev = path.and_then(|p| api.open_path(&p).ok());

        match dev {
            Some(dev) => {
                set_status(&status, &ctx, |s| s.connected = true);
                session(&dev, &status, &ctx, send_keys);
                set_status(&status, &ctx, |s| *s = Status::default());
                thread::sleep(Duration::from_millis(500));
            }
            None => thread::sleep(Duration::from_secs(1)),
        }
    }
}

// ---------- Processo de fundo (bandeja) ----------
fn open_gui(slot: &Arc<Mutex<Option<Child>>>) {
    let mut g = slot.lock().unwrap();
    if let Some(c) = g.as_mut() {
        if matches!(c.try_wait(), Ok(None)) {
            return; // janela já está aberta
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        *g = Command::new(exe).arg("--gui").spawn().ok();
    }
}

fn run_background(open_window: bool, _lock: TcpListener) {
    let status = Arc::new(Mutex::new(Status::default()));
    thread::spawn(move || worker(status, None, true));

    let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));

    let (rgba, w, h) = make_icon();
    let icon = Icon::from_rgba(rgba, w, h).unwrap();

    let menu = Menu::new();
    let open = MenuItem::new("Abrir janela", true, None);
    let quit = MenuItem::new("Sair", true, None);
    menu.append_items(&[&open, &quit]).unwrap();

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("GameSir L4/R4")
        .with_icon(icon)
        .build()
        .unwrap();

    let open_id = open.id().clone();
    let quit_id = quit.id().clone();
    let c1 = child.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == open_id {
            open_gui(&c1);
        } else if e.id == quit_id {
            if let Some(c) = c1.lock().unwrap().as_mut() {
                let _ = c.kill();
            }
            std::process::exit(0);
        }
    }));

    let c2 = child.clone();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::DoubleClick { .. } = e {
            open_gui(&c2);
        }
    }));

    if open_window {
        open_gui(&child);
    }

    // Loop de mensagens do Windows (necessário para o ícone da bandeja funcionar)
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// ---------- Janela ----------
struct App {
    cfg: Arc<Mutex<Config>>,
    status: Arc<Mutex<Status>>,
    capturing: Option<usize>,
    seen_mods: Vec<u16>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let cfg = Arc::new(Mutex::new(load_config()));
        let status = Arc::new(Mutex::new(Status::default()));
        {
            let (s, ctx) = (status.clone(), cc.egui_ctx.clone());
            thread::spawn(move || worker(s, Some(ctx), false));
        }
        Self {
            cfg,
            status,
            capturing: None,
            seen_mods: Vec::new(),
        }
    }

    /// Lê o teclado físico. Retorna Some(teclas) quando o atalho foi definido.
    fn poll_capture(&mut self) -> Option<Vec<u16>> {
        let mods_now: Vec<u16> = MODS.iter().copied().filter(|&m| key_down(m)).collect();
        for &m in &mods_now {
            if !self.seen_mods.contains(&m) {
                self.seen_mods.push(m);
            }
        }
        // Tecla "normal" apertada: finaliza com os modificadores segurados
        for vk in 0x08u16..=0xFE {
            if is_modifier(vk) {
                continue;
            }
            if key_down(vk) {
                let mut keys = mods_now.clone();
                keys.push(vk);
                return Some(keys);
            }
        }
        // Só modificadores e todos foram soltos: atalho de modificador puro
        if mods_now.is_empty() && !self.seen_mods.is_empty() {
            let mut keys = std::mem::take(&mut self.seen_mods);
            keys.sort_by_key(|k| MODS.iter().position(|m| m == k));
            return Some(keys);
        }
        None
    }
}

fn dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 7.0, color);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Captura de tecla em andamento
        if let Some(idx) = self.capturing {
            ctx.request_repaint_after(Duration::from_millis(16));
            if ctx.input(|i| i.focused) {
                if let Some(keys) = self.poll_capture() {
                    let mut c = self.cfg.lock().unwrap();
                    c.buttons[idx].keys = keys;
                    save_config(&c);
                    self.capturing = None;
                }
            }
        }

        let st = *self.status.lock().unwrap();
        let cfg_now = self.cfg.lock().unwrap().clone();
        let capturing = self.capturing;

        let mut start_capture: Option<usize> = None;
        let mut cancel = false;
        let mut clear: Option<usize> = None;
        let mut repeat_change: Option<(usize, bool)> = None;

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("GameSir T3 Lite — L4 / R4");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let (color, text) = if st.connected {
                    (egui::Color32::from_rgb(60, 200, 90), "Controle conectado")
                } else {
                    (
                        egui::Color32::from_rgb(220, 70, 70),
                        "Controle não encontrado",
                    )
                };
                dot(ui, color);
                ui.label(text);
            });
            ui.add_space(8.0);

            for i in 0..2 {
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let color = if st.pressed[i] {
                            egui::Color32::from_rgb(60, 200, 90)
                        } else {
                            egui::Color32::from_gray(90)
                        };
                        dot(ui, color);
                        ui.strong(NAMES[i]);
                        ui.label(if st.pressed[i] { "apertado" } else { "solto" });
                    });

                    let label = if capturing == Some(i) {
                        "Aperte a tecla ou atalho…".to_string()
                    } else if cfg_now.buttons[i].keys.is_empty() {
                        "(nenhuma)".to_string()
                    } else {
                        describe(&cfg_now.buttons[i].keys)
                    };
                    ui.horizontal(|ui| {
                        ui.label("Envia:");
                        ui.monospace(label);
                    });

                    ui.horizontal(|ui| {
                        if capturing == Some(i) {
                            if ui.button("Cancelar").clicked() {
                                cancel = true;
                            }
                        } else {
                            let r = ui.button("Definir tecla / atalho");
                            if r.clicked() {
                                r.surrender_focus();
                                start_capture = Some(i);
                            }
                            if ui.button("Limpar").clicked() {
                                clear = Some(i);
                            }
                        }
                    });

                    let mut rep = cfg_now.buttons[i].repeat;
                    if ui.checkbox(&mut rep, "Repetir enquanto segura").changed() {
                        repeat_change = Some((i, rep));
                    }
                });
                ui.add_space(4.0);
            }

            ui.add_space(4.0);
            ui.weak("Não aperte L4/R4 enquanto define uma tecla.");
        });

        // Aplica as ações depois de desenhar
        if let Some(i) = start_capture {
            self.capturing = Some(i);
            self.seen_mods.clear();
        }
        if cancel {
            self.capturing = None;
            self.seen_mods.clear();
        }
        if let Some(i) = clear {
            let mut c = self.cfg.lock().unwrap();
            c.buttons[i].keys.clear();
            save_config(&c);
        }
        if let Some((i, v)) = repeat_change {
            let mut c = self.cfg.lock().unwrap();
            c.buttons[i].repeat = v;
            save_config(&c);
        }
    }
}

fn run_gui() -> eframe::Result<()> {
    let (rgba, w, h) = make_icon();
    let icon = egui::IconData {
        rgba,
        width: w,
        height: h,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([300.0, 320.0])
            .with_resizable(false)
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native(
        "GameSir L4/R4",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--gui") {
        return run_gui();
    }

    let tray_only = args.iter().any(|a| a == "--tray");

    // Instância única do processo de fundo
    match TcpListener::bind(LOCK_ADDR) {
        Ok(lock) => run_background(!tray_only, lock),
        Err(_) => {
            // Já existe um processo de fundo: apenas abre a janela
            if !tray_only {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = Command::new(exe).arg("--gui").spawn();
                }
            }
        }
    }
    Ok(())
}
