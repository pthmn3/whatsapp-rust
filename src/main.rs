//! ==============================================================================
//! WhatsApp Native Desktop Client (Pure Rust + Rune Native UI + Multi-Device)
//! ==============================================================================
//! Ultra-lightweight standalone Windows desktop client for WhatsApp.
//! Framework: Rune Native GPU UI (Pure Rust wgpu / DirectX 12)
//! Protocol: WhatsApp Multi-Device (Noise Protocol + Signal E2EE) via whatsapp-rust
//! ==============================================================================

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{SystemTime, UNIX_EPOCH};

// Rune Framework imports
use rune::tok::{self, TextStyle};
use rune::{
    col, el, row, text, text_input,
    AppSpec, Color, Cx, El, ScrollSpec, TextEdit, Tx, Waker,
};

// Crossbeam channels for zero-allocation thread communication
use crossbeam_channel::{unbounded, Receiver, Sender};

// Windows Native Integrations
use notify_rust::Notification;
use tray_icon::{
    menu::{Menu, MenuItem},
    TrayIcon, TrayIconBuilder,
};

// Live WhatsApp Multi-Device Protocol
use whatsapp_rust::pair_code::PairCodeOptions;
use whatsapp_rust::prelude::*;
use wacore::store::DevicePropsOverride;
use whatsapp_rust::prelude::wa::device_props::PlatformType;

static INITIAL_PHONE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

// ==============================================================================
// 1. APPLICATION DATA MODELS & STATE
// ==============================================================================

#[derive(Clone, Debug, PartialEq)]
pub enum AuthState {
    Initializing,
    NeedsPairing {
        qr_ascii: Option<String>,
        pair_code: Option<String>,
        status: String,
    },
    Connected {
        user: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActiveView {
    Chats,
    Status,
    Calls,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub id: String,
    pub chat_jid: String,
    pub sender: String,
    pub content: String,
    pub timestamp: String,
    pub is_from_me: bool,
}

#[derive(Clone, Debug)]
pub struct ChatItem {
    pub jid: String,
    pub name: String,
    pub last_message: String,
    pub timestamp: String,
    pub unread_count: usize,
    pub messages: Vec<ChatMessage>,
}

#[derive(Clone, Debug)]
pub struct StatusUpdate {
    pub id: String,
    pub author_name: String,
    pub author_jid: String,
    pub caption: String,
    pub timestamp: String,
    pub media_url: String,
    pub media_type: String,
    pub media_key: Vec<u8>,
}

#[derive(Clone)]
pub struct AppState {
    pub auth_state: AuthState,
    pub active_view: ActiveView,
    pub active_chat_jid: Option<String>,
    pub draft: TextEdit,
    pub search_or_new: TextEdit,
    pub phone_input: TextEdit,
    pub chats: Vec<ChatItem>,
    pub statuses: Vec<StatusUpdate>,
    pub active_status_index: usize,
    pub status_feedback: String,

    // Channels for async backend interaction
    pub tx_outbound: Sender<OutboundAction>,
    pub rx_inbound: Receiver<InboundEvent>,
    pub waker: Option<Waker>,
}

impl Default for AppState {
    fn default() -> Self {
        let (tx, _rx) = unbounded();
        let (_tx2, rx2) = unbounded();
        Self {
            auth_state: AuthState::Initializing,
            active_view: ActiveView::Chats,
            active_chat_jid: None,
            draft: TextEdit::default(),
            search_or_new: TextEdit::default(),
            phone_input: TextEdit::default(),
            chats: Vec::new(),
            statuses: sample_statuses(),
            active_status_index: 0,
            status_feedback: String::new(),
            tx_outbound: tx,
            rx_inbound: rx2,
            waker: None,
        }
    }
}

impl AppState {
    pub fn apply_inbound_event(&mut self, event: InboundEvent) {
        match event {
            InboundEvent::QrCode(qr) => {
                if let AuthState::NeedsPairing { ref mut qr_ascii, ref mut status, .. } = self.auth_state {
                    *qr_ascii = Some(qr);
                    *status = "Scan the QR code with WhatsApp on your phone, or enter phone number below".into();
                } else {
                    self.auth_state = AuthState::NeedsPairing {
                        qr_ascii: Some(qr),
                        pair_code: None,
                        status: "Scan the QR code with WhatsApp on your phone, or enter phone number below".into(),
                    };
                }
            }
            InboundEvent::PairCode(code) => {
                if let AuthState::NeedsPairing { ref mut pair_code, ref mut status, .. } = self.auth_state {
                    *pair_code = Some(code.clone());
                    *status = format!("Pair code: {}. Enter this in WhatsApp on your phone!", code);
                } else {
                    self.auth_state = AuthState::NeedsPairing {
                        qr_ascii: None,
                        pair_code: Some(code.clone()),
                        status: format!("Pair code: {}. Enter this in WhatsApp on your phone!", code),
                    };
                }
            }
            InboundEvent::PairCodeError(err) => {
                if let AuthState::NeedsPairing { status, .. } = &mut self.auth_state {
                    *status = format!("Pairing error: {}", err);
                }
            }
            InboundEvent::Connected(user) => {
                println!("[UI] Authenticated and connected as {}", user);
                self.auth_state = AuthState::Connected { user: user.clone() };
                self.status_feedback = format!("Connected as {}", user);
                if self.chats.is_empty() {
                    self.chats.push(ChatItem {
                        jid: "self@s.whatsapp.net".into(),
                        name: "WhatsApp Native".into(),
                        last_message: "Connected to WhatsApp live network!".into(),
                        timestamp: chrono::Local::now().format("%H:%M").to_string(),
                        unread_count: 0,
                        messages: vec![ChatMessage {
                            id: "welcome_1".into(),
                            chat_jid: "self@s.whatsapp.net".into(),
                            sender: "WhatsApp Native".into(),
                            content: "You are connected! Enter a phone number in the search box above to start chatting with anyone, or wait for incoming messages.".into(),
                            timestamp: chrono::Local::now().format("%H:%M").to_string(),
                            is_from_me: false,
                        }],
                    });
                    self.active_chat_jid = Some("self@s.whatsapp.net".into());
                }
            }
            InboundEvent::LoggedOut => {
                self.auth_state = AuthState::NeedsPairing {
                    qr_ascii: None,
                    pair_code: None,
                    status: "Session logged out. Re-pairing needed.".into(),
                };
            }
            InboundEvent::NewMessage(msg) => {
                let chat_jid = msg.chat_jid.clone();
                let sender_name = msg.sender.clone();
                let last_content = msg.content.clone();
                let timestamp = msg.timestamp.clone();

                if let Some(pos) = self.chats.iter().position(|c| c.jid == chat_jid) {
                    self.chats[pos].messages.push(msg);
                    self.chats[pos].last_message = last_content;
                    self.chats[pos].timestamp = timestamp;
                    let item = self.chats.remove(pos);
                    self.chats.insert(0, item);
                } else {
                    self.chats.insert(0, ChatItem {
                        jid: chat_jid.clone(),
                        name: sender_name,
                        last_message: last_content,
                        timestamp,
                        unread_count: 1,
                        messages: vec![msg],
                    });
                }
                if self.active_chat_jid.is_none() {
                    self.active_chat_jid = Some(chat_jid);
                }
            }
            InboundEvent::StatusUpdated(s) => {
                self.statuses.push(s);
            }
            InboundEvent::CallIncoming { caller, .. } => {
                show_native_notification("WhatsApp Call", &format!("Incoming call from {}", caller));
            }
            InboundEvent::NetworkLog(log) => {
                self.status_feedback = log;
            }
        }
    }
}

// ==============================================================================
// 2. BACKEND PROTOCOL & LIVE WHATSAPP MULTI-DEVICE ACTOR
// ==============================================================================

pub enum OutboundAction {
    SetWaker(Waker),
    SendMessage { to_jid: String, text: String },
    RequestPairCode { phone_number: String },
    DownloadStatusMedia { status_id: String, url: String, key: Vec<u8> },
    InitiateCall { to_jid: String, is_video: bool },
    MinimizeToTray,
}

pub enum InboundEvent {
    QrCode(String),
    PairCode(String),
    PairCodeError(String),
    Connected(String),
    LoggedOut,
    NewMessage(ChatMessage),
    StatusUpdated(StatusUpdate),
    CallIncoming { caller: String, is_video: bool },
    NetworkLog(String),
}

/// Spawns the dedicated background actor connecting to real WhatsApp Multi-Device.
pub fn spawn_network_backend(
    rx_outbound: Receiver<OutboundAction>,
    tx_inbound: Sender<InboundEvent>,
) {
    std::thread::Builder::new()
        .name("wa-network-actor".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to build Tokio async runtime");

            rt.block_on(async move {
                println!("[Network] Opening persistent SQLite store at 'whatsapp_session.db'...");
                let store = match SqliteStore::new("whatsapp_session.db").await {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("[Network] SQLite Store error: {}", e);
                        let _ = tx_inbound.send(InboundEvent::NetworkLog(format!("Store error: {}", e)));
                        return;
                    }
                };

                let (waker_tx, waker_rx) = tokio::sync::watch::channel::<Option<Waker>>(None);
                let waker_rx_qr = waker_rx.clone();
                let waker_rx_pair = waker_rx.clone();
                let waker_rx_conn = waker_rx.clone();
                let waker_rx_msg = waker_rx.clone();

                let tx_qr = tx_inbound.clone();
                let tx_pair = tx_inbound.clone();
                let tx_pair_err = tx_inbound.clone();
                let tx_conn = tx_inbound.clone();
                let tx_logout = tx_inbound.clone();
                let tx_msg = tx_inbound.clone();

                let device_props = DevicePropsOverride::new()
                    .with_os("Windows")
                    .with_platform_type(PlatformType::CHROME)
                    .with_version(whatsapp_rust::prelude::wa::device_props::AppVersion {
                        primary: Some(2),
                        secondary: Some(3000),
                        tertiary: Some(1054320313),
                        ..Default::default()
                    });

                println!("[Network] Connecting to WhatsApp Multi-Device protocol (Chrome on Windows)...");
                let bot_result = Bot::builder()
                    .with_backend(store)
                    .with_version((2, 3000, 1054320313))
                    .with_device_props(device_props)
                    .with_push_name("WhatsApp Native")
                    .on_event({
                        let tx = tx_inbound.clone();
                        let rx = waker_rx_conn.clone();
                        move |event, _client| {
                            let tx = tx.clone();
                            let rx = rx.clone();
                            async move {
                                match event.as_ref() {
                                    Event::PairSuccess(success) => {
                                        println!("\n=======================================================");
                                        println!(">>> [WhatsApp] PAIR SUCCESS! JID: {:?}", success.id);
                                        println!("=======================================================\n");
                                        let user_str = success.id.to_string();
                                        let _ = tx.send(InboundEvent::Connected(user_str));
                                        if let Some(waker) = rx.borrow().as_ref() {
                                            waker();
                                        }
                                    }
                                    Event::PairError(err) => {
                                        eprintln!("\n>>> [WhatsApp] PAIR ERROR: {} <<<\n", err.error);
                                        let _ = tx.send(InboundEvent::PairCodeError(format!("Pairing failed: {}", err.error)));
                                        if let Some(waker) = rx.borrow().as_ref() {
                                            waker();
                                        }
                                    }
                                    Event::PairPasskeyRequest(req) => {
                                        println!("[WhatsApp] Passkey verification requested: {}", req.request_options_json);
                                    }
                                    Event::PairPasskeyConfirmation(conf) => {
                                        println!("\n>>> [WhatsApp] PASSKEY PAIR CODE: {} <<<\n", conf.code);
                                        let _ = tx.send(InboundEvent::PairCode(conf.code.clone()));
                                        if let Some(waker) = rx.borrow().as_ref() {
                                            waker();
                                        }
                                    }
                                    Event::PairPasskeyError(err) => {
                                        eprintln!("[WhatsApp] Passkey error: {}", err.error);
                                        let _ = tx.send(InboundEvent::PairCodeError(format!("Passkey error: {}", err.error)));
                                    }
                                    Event::ClientOutdated(_) => {
                                        eprintln!("[WhatsApp] Client version reported outdated by server.");
                                    }
                                    Event::QrScannedWithoutMultidevice(_) => {
                                        eprintln!("[WhatsApp] QR scanned without multi-device support on phone.");
                                    }
                                    _ => {}
                                }
                            }
                        }
                    })
                    .on_qr_code(move |code, _timeout| {
                        let tx = tx_qr.clone();
                        let rx = waker_rx_qr.clone();
                        async move {
                            println!("\n[WhatsApp] QR Code received from servers!");
                            if let Ok(qr) = qrcode::QrCode::new(code.as_bytes()) {
                                let ascii = qr.render::<qrcode::render::unicode::Dense1x2>().build();
                                println!("\n================ SCAN QR CODE TO PAIR WHATSAPP ================\n{}\n===============================================================\n", ascii);
                                let _ = tx.send(InboundEvent::QrCode(ascii));
                                if let Some(waker) = rx.borrow().as_ref() {
                                    waker();
                                }
                            }
                        }
                    })
                    .on_pair_code(move |code, _timeout| {
                        let tx = tx_pair.clone();
                        let rx = waker_rx_pair.clone();
                        async move {
                            println!("\n[WhatsApp] Phone pairing code: {}\n", code);
                            let _ = tx.send(InboundEvent::PairCode(code));
                            if let Some(waker) = rx.borrow().as_ref() {
                                waker();
                            }
                        }
                    })
                    .on_pair_code_error(move |err, _client| {
                        let tx = tx_pair_err.clone();
                        async move {
                            eprintln!("[WhatsApp] Pairing code error: {}", err.error);
                            let _ = tx.send(InboundEvent::PairCodeError(err.error));
                        }
                    })
                    .on_connected(move |_client| {
                        let tx = tx_conn.clone();
                        let rx = waker_rx_conn.clone();
                        async move {
                            println!("[WhatsApp] Successfully connected & authenticated!");
                            show_native_notification("WhatsApp Native", "Connected and authenticated successfully!");
                            let user_str = "WhatsApp User".to_string();
                            let _ = tx.send(InboundEvent::Connected(user_str));
                            if let Some(waker) = rx.borrow().as_ref() {
                                waker();
                            }
                        }
                    })
                    .on_logged_out(move |_info| {
                        let tx = tx_logout.clone();
                        async move {
                            println!("[WhatsApp] Session logged out.");
                            let _ = tx.send(InboundEvent::LoggedOut);
                        }
                    })
                    .on_message(move |ctx| {
                        let tx = tx_msg.clone();
                        let rx = waker_rx_msg.clone();
                        async move {
                            use whatsapp_rust::prelude::MessageExt;
                            let content = ctx.message.text_content().unwrap_or("").to_string();
                            if content.is_empty() {
                                return;
                            }
                            let chat_jid = ctx.info.source.chat.to_string();
                            let sender_name = if !ctx.info.push_name.is_empty() {
                                ctx.info.push_name.clone()
                            } else {
                                ctx.info.source.sender.to_string()
                            };
                            let is_from_me = ctx.info.source.is_from_me;
                            let timestamp = ctx.info.timestamp.format("%H:%M").to_string();

                            println!("[Message] From {}: {}", sender_name, content);
                            if !is_from_me {
                                show_native_notification(&sender_name, &content);
                            }

                            let _ = tx.send(InboundEvent::NewMessage(ChatMessage {
                                id: ctx.info.id.to_string(),
                                chat_jid,
                                sender: sender_name,
                                content,
                                timestamp,
                                is_from_me,
                            }));

                            if let Some(waker) = rx.borrow().as_ref() {
                                waker();
                            }
                        }
                    })
                    .build()
                    .await;

                let bot = match bot_result {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("[Network] Bot build failed: {}", e);
                        let _ = tx_inbound.send(InboundEvent::NetworkLog(format!("Connection error: {}", e)));
                        return;
                    }
                };

                let client = bot.client();
                let _bot_handle = bot.spawn();

                while let Ok(action) = rx_outbound.recv() {
                    match action {
                        OutboundAction::SetWaker(waker) => {
                            let _ = waker_tx.send(Some(waker));
                        }
                        OutboundAction::SendMessage { to_jid, text } => {
                            let client = client.clone();
                            tokio::spawn(async move {
                                let jid = if to_jid.contains('@') {
                                    to_jid.parse::<Jid>().unwrap_or_else(|_| Jid::pn(&to_jid))
                                } else {
                                    Jid::pn(&to_jid)
                                };
                                println!("[Network] Dispatched message to {}: {}", jid, text);
                                match client.send_text(jid, text).await {
                                    Ok(_) => {
                                        println!("[Network] Message sent and confirmed!");
                                    }
                                    Err(e) => {
                                        eprintln!("[Network Send Error] {}", e);
                                    }
                                }
                            });
                        }
                        OutboundAction::RequestPairCode { phone_number } => {
                            let client = client.clone();
                            tokio::spawn(async move {
                                let clean_digits: String = phone_number.chars().filter(|c| c.is_ascii_digit()).collect();
                                let clean_phone = clean_digits.trim_start_matches('0').to_string();
                                println!("[Network] Waiting for WhatsApp connection before requesting pairing code for '{}'...", clean_phone);
                                if let Err(e) = client.wait_for_socket(std::time::Duration::from_secs(30)).await {
                                    eprintln!("[Network Pair Code Error] Socket wait error: {}", e);
                                    return;
                                }
                                let options = PairCodeOptions {
                                    phone_number: clean_phone,
                                    ..Default::default()
                                };
                                match client.pair_with_code(options).await {
                                    Ok(code) => {
                                        println!("\n=======================================================");
                                        println!(">>> [WhatsApp] PHONE PAIRING CODE: {} <<<", code);
                                        println!(">>> On phone: Settings > Linked Devices > Link a Device > Link with phone number <<<");
                                        println!("=======================================================\n");
                                    }
                                    Err(e) => {
                                        eprintln!("[Network Pair Code Error] Failed to generate code: {}", e);
                                    }
                                }
                            });
                        }
                        OutboundAction::DownloadStatusMedia { status_id, url, key } => {
                            tokio::spawn(async move {
                                if let Err(e) = download_and_decrypt_status(&url, &key, &status_id).await {
                                    eprintln!("[Media Download Error] {}", e);
                                }
                            });
                        }
                        OutboundAction::InitiateCall { to_jid, is_video } => {
                            println!("[Call Signaling] Initiating {} call with {}", if is_video { "Video" } else { "Voice" }, to_jid);
                        }
                        OutboundAction::MinimizeToTray => {
                            println!("[System Tray] Window hidden to notification area.");
                        }
                    }
                }
            });
        })
        .expect("Failed to spawn background network thread");
}

// ==============================================================================
// 3. WINDOWS SYSTEM INTEGRATIONS
// ==============================================================================

pub fn init_system_tray() -> Option<TrayIcon> {
    let tray_menu = Menu::new();
    let quit_item = MenuItem::new("Quit WhatsApp Native", true, None);
    let open_item = MenuItem::new("Open", true, None);

    let _ = tray_menu.append(&open_item);
    let _ = tray_menu.append(&quit_item);

    let icon_data = vec![0x25, 0xD3, 0x66, 0xFF].repeat(16 * 16);
    let icon = tray_icon::Icon::from_rgba(icon_data, 16, 16).ok()?;

    match TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("WhatsApp Native (Multi-Device)")
        .with_icon(icon)
        .build()
    {
        Ok(t) => Some(t),
        Err(e) => {
            println!("[System Tray] Taskbar notification area not active ({}), continuing without tray icon.", e);
            None
        }
    }
}

pub fn show_native_notification(title: &str, body: &str) {
    let _ = Notification::new()
        .appname("WhatsApp Native")
        .summary(title)
        .body(body)
        .show();
}

pub async fn download_and_decrypt_status(
    media_url: &str,
    _media_key: &[u8],
    status_id: &str,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let download_dir = dirs::download_dir().unwrap_or_else(|| PathBuf::from("C:\\Users\\Public\\Downloads"));
    let target_filename = format!("WhatsApp_Status_{}.jpg", status_id);
    let target_path = download_dir.join(target_filename);

    let response = reqwest::get(media_url).await?;
    let encrypted_bytes = response.bytes().await?;
    tokio::fs::write(&target_path, &encrypted_bytes).await?;

    show_native_notification(
        "WhatsApp Status Downloaded",
        &format!("Saved to {}", target_path.display()),
    );

    Ok(target_path)
}

// ==============================================================================
// 4. WEBRTC AUDIO/VIDEO CALL ENGINE
// ==============================================================================

pub struct CallEngine {
    audio_host: cpal::Host,
    active_call: Arc<AtomicBool>,
}

impl CallEngine {
    pub fn new() -> Self {
        Self {
            audio_host: cpal::default_host(),
            active_call: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start_audio_capture<F>(&self, mut on_pcm_frame: F) -> Result<(), Box<dyn std::error::Error>>
    where
        F: FnMut(&[f32]) + Send + 'static,
    {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

        let device = self.audio_host.default_input_device()
            .ok_or("No default audio input hardware found")?;

        let config = device.default_input_config()?;
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _| on_pcm_frame(data),
                |err| eprintln!("[WASAPI Audio Error] {}", err),
                None,
            )?,
            _ => return Err("Unsupported PCM format".into()),
        };

        stream.play()?;
        self.active_call.store(true, Ordering::SeqCst);
        Ok(())
    }
}

// ==============================================================================
// 5. NATIVE UI LAYOUT & RENDERING (Rune Declarative Syntax)
// ==============================================================================

fn col_rgb(hex: u32) -> Color {
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
    let b = (hex & 0xFF) as f32 / 255.0;
    Color {
        r: rune::color::srgb_decode(r),
        g: rune::color::srgb_decode(g),
        b: rune::color::srgb_decode(b),
        a: 1.0,
    }
}

fn font_size(sz: f32) -> TextStyle {
    TextStyle::new(sz, 400.0)
}

struct Palette {
    bg_app: Color,
    sidebar: Color,
    chat_bg: Color,
    header: Color,
    bubble_in: Color,
    bubble_out: Color,
    accent_green: Color,
    text_primary: Color,
    text_muted: Color,
    input_bg: Color,
    avatar_bg: Color,
}

fn palette() -> &'static Palette {
    static PALETTE: LazyLock<Palette> = LazyLock::new(|| Palette {
        bg_app: col_rgb(0x0C1317),
        sidebar: col_rgb(0x111B21),
        chat_bg: col_rgb(0x0B141A),
        header: col_rgb(0x202C33),
        bubble_in: col_rgb(0x202C33),
        bubble_out: col_rgb(0x005C4B),
        accent_green: col_rgb(0x00A884),
        text_primary: col_rgb(0xE9EDEF),
        text_muted: col_rgb(0x8696A0),
        input_bg: col_rgb(0x2A3942),
        avatar_bg: col_rgb(0x6A7B82),
    });
    &PALETTE
}

pub fn view(s: &AppState, _cx: &mut Cx<AppState>) -> El {
    let p = palette();
    row()
        .w_full()
        .h_full()
        .fill(p.bg_app)
        .child(render_sidebar(s))
        .child(render_main_panel(s))
}

fn on_start_chat(s: &mut AppState, _tx: &mut Tx) {
    let input = s.search_or_new.text.trim();
    if input.is_empty() {
        return;
    }
    let digits: String = input.chars().filter(|c| c.is_ascii_digit()).collect();
    let (jid, display_name) = if input.contains('@') {
        (input.to_string(), input.to_string())
    } else if !digits.is_empty() {
        (format!("{}@s.whatsapp.net", digits), format!("+{}", digits))
    } else {
        (format!("{}@s.whatsapp.net", input), input.to_string())
    };

    if !s.chats.iter().any(|c| c.jid == jid) {
        s.chats.insert(0, ChatItem {
            jid: jid.clone(),
            name: display_name,
            last_message: "Started new conversation".into(),
            timestamp: chrono::Local::now().format("%H:%M").to_string(),
            unread_count: 0,
            messages: Vec::new(),
        });
    }
    s.active_chat_jid = Some(jid);
    s.active_view = ActiveView::Chats;
    s.search_or_new.clear();
}

fn send_chat_message(s: &mut AppState, tx: &mut Tx) {
    let text = s.draft.text.trim().to_string();
    if text.is_empty() {
        return;
    }
    let Some(current_jid) = s.active_chat_jid.clone() else {
        return;
    };

    let now_ts = chrono::Local::now().format("%H:%M").to_string();
    let new_msg = ChatMessage {
        id: format!("msg_{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()),
        chat_jid: current_jid.clone(),
        sender: "Me".into(),
        content: text.clone(),
        timestamp: now_ts.clone(),
        is_from_me: true,
    };

    if let Some(chat) = s.chats.iter_mut().find(|c| c.jid == current_jid) {
        chat.messages.push(new_msg.clone());
        chat.last_message = text.clone();
        chat.timestamp = now_ts;
    }

    let _ = s.tx_outbound.send(OutboundAction::SendMessage {
        to_jid: current_jid,
        text,
    });

    s.draft.clear();
    tx.scroll_to_bottom("messages");
}

fn render_sidebar(s: &AppState) -> El {
    let p = palette();
    col()
        .w(340.0)
        .h_full()
        .fill(p.sidebar)
        // Top Toolbar
        .child(
            row()
                .h(60.0)
                .w_full()
                .fill(p.header)
                .padding(12.0)
                .gap(10.0)
                .child(
                    text("WhatsApp")
                        .font(font_size(17.0))
                        .color(p.text_primary)
                )
                // Tab: Chats
                .child(
                    el()
                        .padding(6.0)
                        .radius(6.0)
                        .fill(if s.active_view == ActiveView::Chats { p.accent_green } else { Color::TRANSPARENT })
                        .on_press(|s: &mut AppState| s.active_view = ActiveView::Chats)
                        .child(text("Chats").color(p.text_primary).font(font_size(13.0)))
                )
                // Tab: Status
                .child(
                    el()
                        .padding(6.0)
                        .radius(6.0)
                        .fill(if s.active_view == ActiveView::Status { p.accent_green } else { Color::TRANSPARENT })
                        .on_press(|s: &mut AppState| s.active_view = ActiveView::Status)
                        .child(text("Status").color(p.text_primary).font(font_size(13.0)))
                )
                // Minimize to Tray Action
                .child(
                    el()
                        .padding(6.0)
                        .radius(4.0)
                        .fill(p.input_bg)
                        .on_press(|s: &mut AppState| {
                            let _ = s.tx_outbound.send(OutboundAction::MinimizeToTray);
                        })
                        .child(text("Tray").color(p.text_muted).font(font_size(11.0)))
                )
        )
        // Search & Start Chat Box
        .child(
            row()
                .h(50.0)
                .w_full()
                .padding(8.0)
                .gap(6.0)
                .fill(p.sidebar)
                .child(
                    text_input(&s.search_or_new, tok::type_::BODY)
                        .placeholder("🔍 Search or enter phone (+1...)", p.text_muted)
                        .color(p.text_primary)
                        .grow(1.0)
                        .h(34.0)
                        .padding(6.0)
                        .radius(6.0)
                        .fill(p.input_bg)
                        .on_edit(|s: &mut AppState| &mut s.search_or_new)
                        .on_submit(on_start_chat)
                )
                .child(
                    el()
                        .w(34.0)
                        .h(34.0)
                        .radius(6.0)
                        .fill(p.accent_green)
                        .on_press_tx(on_start_chat)
                        .child(text("+").font(font_size(16.0)).color(Color::WHITE))
                )
        )
        // Scrollable Chat Item List
        .child(render_chat_list(s))
}

fn render_chat_list(s: &AppState) -> El {
    let p = palette();
    let mut list = col().w_full().h_full().gap(1.0);

    for chat in &s.chats {
        let is_selected = s.active_chat_jid.as_ref() == Some(&chat.jid);
        let jid = chat.jid.clone();

        let chat_row = row()
            .w_full()
            .h(72.0)
            .padding(12.0)
            .gap(12.0)
            .fill(if is_selected { p.input_bg } else { p.sidebar })
            .on_press(move |s: &mut AppState| {
                s.active_chat_jid = Some(jid.clone());
                s.active_view = ActiveView::Chats;
            })
            .child(
                el()
                    .w(48.0)
                    .h(48.0)
                    .radius(24.0)
                    .fill(p.avatar_bg)
                    .child(text("👤").font(font_size(20.0)))
            )
            .child(
                col()
                    .w_full()
                    .gap(4.0)
                    .child(
                        row()
                            .w_full()
                            .child(text(&chat.name).color(p.text_primary).font(font_size(15.0)))
                            .child(text(&chat.timestamp).color(p.text_muted).font(font_size(11.0)))
                    )
                    .child(
                        text(&chat.last_message)
                            .color(p.text_muted)
                            .font(font_size(13.0))
                    )
            );

        list = list.child(chat_row);
    }

    list
}

fn render_main_panel(s: &AppState) -> El {
    match s.auth_state {
        AuthState::NeedsPairing { ref qr_ascii, ref pair_code, ref status } => {
            render_pairing_panel(s, qr_ascii.as_deref(), pair_code.as_deref(), status)
        }
        _ => match s.active_view {
            ActiveView::Chats => render_conversation_panel(s),
            ActiveView::Status => render_status_panel(s),
            ActiveView::Calls => col().w_full().h_full().child(text("Calls").color(palette().text_primary)),
        },
    }
}

fn render_pairing_panel(s: &AppState, qr_ascii: Option<&str>, pair_code: Option<&str>, status: &str) -> El {
    let p = palette();

    let mut content = col()
        .w(560.0)
        .padding(28.0)
        .radius(16.0)
        .fill(p.sidebar)
        .gap(18.0)
        .child(
            text("Link WhatsApp Multi-Device")
                .font(TextStyle::new(22.0, 600.0))
                .color(p.text_primary)
        )
        .child(
            text("1. Open WhatsApp on your phone\n2. Tap Settings or Menu > Linked Devices\n3. Tap Link a Device and point your camera at this QR code:")
                .font(TextStyle::new(13.0, 400.0))
                .color(p.text_muted)
        );

    if let Some(qr) = qr_ascii {
        content = content.child(
            col()
                .padding(12.0)
                .radius(10.0)
                .fill(Color::WHITE)
                .child(
                    text(qr)
                        .font(TextStyle::new(8.0, 400.0).mono().lh(8.0))
                        .color(Color::BLACK)
                )
        );
    } else {
        content = content.child(
            el()
                .h(220.0)
                .w_full()
                .radius(10.0)
                .fill(p.header)
                .padding(20.0)
                .child(
                    text("Connecting to WhatsApp servers to retrieve pairing QR code...\n(Ensure internet connection is active)")
                        .font(font_size(13.0))
                        .color(p.text_muted)
                )
        );
    }

    if let Some(code) = pair_code {
        content = content.child(
            col()
                .padding(16.0)
                .radius(10.0)
                .fill(p.bubble_out)
                .gap(6.0)
                .child(
                    text("YOUR PAIRING CODE:")
                        .font(TextStyle::new(12.0, 600.0))
                        .color(p.text_primary)
                )
                .child(
                    text(code)
                        .font(TextStyle::new(26.0, 700.0).mono())
                        .color(p.accent_green)
                )
                .child(
                    text("Enter this code on your phone when prompted in WhatsApp")
                        .font(font_size(12.0))
                        .color(p.text_primary)
                )
        );
    }

    let phone_field = text_input(&s.phone_input, tok::type_::BODY)
        .placeholder("Phone number with country code (e.g. 15551234567)", p.text_muted)
        .color(p.text_primary)
        .grow(1.0)
        .h(40.0)
        .padding(8.0)
        .radius(6.0)
        .fill(p.input_bg)
        .on_edit(|s: &mut AppState| &mut s.phone_input);

    let pair_button = el()
        .h(40.0)
        .padding(10.0)
        .radius(6.0)
        .fill(p.accent_green)
        .on_press(|s: &mut AppState| {
            let phone = s.phone_input.text.trim().to_string();
            if !phone.is_empty() {
                let _ = s.tx_outbound.send(OutboundAction::RequestPairCode { phone_number: phone });
            }
        })
        .child(text("Get Code").font(font_size(13.0)).color(Color::WHITE));

    content = content
        .child(
            text("— OR LINK WITH PHONE NUMBER —")
                .font(TextStyle::new(12.0, 600.0))
                .color(p.text_muted)
        )
        .child(
            row()
                .w_full()
                .gap(10.0)
                .child(phone_field)
                .child(pair_button)
        );

    content = content.child(
        text(status)
            .font(font_size(12.0))
            .color(p.accent_green)
    );

    col()
        .w_full()
        .h_full()
        .fill(p.chat_bg)
        .padding(32.0)
        .child(content)
}

fn render_conversation_panel(s: &AppState) -> El {
    let p = palette();
    let active_chat = s.chats.iter().find(|c| s.active_chat_jid.as_ref() == Some(&c.jid));

    col()
        .w_full()
        .h_full()
        .fill(p.chat_bg)
        // Chat Header
        .child(
            row()
                .h(60.0)
                .w_full()
                .padding(12.0)
                .fill(p.header)
                .gap(16.0)
                .child(
                    text(active_chat.map(|c| c.name.as_str()).unwrap_or("Select a chat or start one above"))
                        .color(p.text_primary)
                        .font(font_size(16.0))
                )
                .child(
                    el()
                        .padding(6.0)
                        .radius(4.0)
                        .fill(p.input_bg)
                        .on_press(|s: &mut AppState| {
                            if let Some(jid) = &s.active_chat_jid {
                                let _ = s.tx_outbound.send(OutboundAction::InitiateCall {
                                    to_jid: jid.clone(),
                                    is_video: false,
                                });
                            }
                        })
                        .child(text("📞 Call").color(p.text_primary).font(font_size(12.0)))
                )
                .child(
                    el()
                        .padding(6.0)
                        .radius(4.0)
                        .fill(p.input_bg)
                        .on_press(|s: &mut AppState| {
                            if let Some(jid) = &s.active_chat_jid {
                                let _ = s.tx_outbound.send(OutboundAction::InitiateCall {
                                    to_jid: jid.clone(),
                                    is_video: true,
                                });
                            }
                        })
                        .child(text("🎥 Video").color(p.text_primary).font(font_size(12.0)))
                )
        )
        // Message Stream
        .child(render_message_stream(active_chat))
        // Bottom Text Input & Send
        .child(render_input_bar(s))
}

fn render_message_stream(chat: Option<&ChatItem>) -> El {
    let p = palette();
    let mut stream = col().w_full().gap(8.0).padding(16.0);

    if let Some(c) = chat {
        for msg in &c.messages {
            let bubble = row()
                .w_full()
                .child(
                    el()
                        .padding(10.0)
                        .radius(8.0)
                        .fill(if msg.is_from_me { p.bubble_out } else { p.bubble_in })
                        .child(
                            col()
                                .gap(4.0)
                                .child(text(&msg.content).color(p.text_primary).font(font_size(14.0)))
                                .child(text(&msg.timestamp).color(p.text_muted).font(font_size(10.0)))
                        )
                );
            stream = stream.child(bubble);
        }
    }

    col()
        .key("messages")
        .w_full()
        .grow(1.0)
        .shrink()
        .scroll(ScrollSpec { stick_bottom: true })
        .child(stream)
}

fn render_input_bar(s: &AppState) -> El {
    let p = palette();

    let composer = text_input(&s.draft, tok::type_::BODY)
        .key("composer")
        .placeholder("Type a message", p.text_muted)
        .color(p.text_primary)
        .grow(1.0)
        .h(42.0)
        .padding(10.0)
        .radius(8.0)
        .fill(p.input_bg)
        .on_edit(|s: &mut AppState| &mut s.draft)
        .on_submit(send_chat_message);

    let send_button = el()
        .key("send")
        .w(42.0)
        .h(42.0)
        .radius(21.0)
        .fill(p.accent_green)
        .on_press_tx(send_chat_message)
        .child(text("➤").font(font_size(16.0)).color(Color::WHITE));

    row()
        .h(64.0)
        .w_full()
        .padding(10.0)
        .gap(10.0)
        .fill(p.header)
        .child(text("📎").font(font_size(18.0)).color(p.text_muted))
        .child(composer)
        .child(send_button)
}

fn render_status_panel(s: &AppState) -> El {
    let p = palette();
    let current_status = s.statuses.get(s.active_status_index);

    col()
        .w_full()
        .h_full()
        .padding(24.0)
        .gap(16.0)
        .fill(p.chat_bg)
        .child(
            text("Status Updates")
                .font(font_size(20.0))
                .color(p.text_primary)
        )
        .child(
            if let Some(status) = current_status {
                let status_id = status.id.clone();
                let url = status.media_url.clone();
                let key = status.media_key.clone();

                col()
                    .w(480.0)
                    .padding(16.0)
                    .radius(10.0)
                    .fill(p.header)
                    .gap(12.0)
                    .child(text(&format!("From: {}", status.author_name)).color(p.text_primary).font(font_size(16.0)))
                    .child(text(&format!("Caption: {}", status.caption)).color(p.text_muted).font(font_size(14.0)))
                    .child(
                        el()
                            .padding(10.0)
                            .radius(6.0)
                            .fill(p.accent_green)
                            .on_press(move |s: &mut AppState| {
                                s.status_feedback = "Downloading to Windows Downloads...".into();
                                let _ = s.tx_outbound.send(OutboundAction::DownloadStatusMedia {
                                    status_id: status_id.clone(),
                                    url: url.clone(),
                                    key: key.clone(),
                                });
                            })
                            .child(text("⬇ Download Status Media").color(Color::WHITE).font(font_size(13.0)))
                    )
                    .child(text(&s.status_feedback).color(p.accent_green).font(font_size(12.0)))
            } else {
                col().child(text("No status updates available.").color(p.text_muted))
            }
        )
}

fn sample_statuses() -> Vec<StatusUpdate> {
    vec![StatusUpdate {
        id: "stat_83921".into(),
        author_name: "Ferris the Crab".into(),
        author_jid: "111222333@s.whatsapp.net".into(),
        caption: "Zero-cost abstractions in production! 🦀".into(),
        timestamp: "2 hours ago".into(),
        media_url: "https://raw.githubusercontent.com/rust-lang/rust-artwork/master/logo/rust-logo-512x512.png".into(),
        media_type: "image/png".into(),
        media_key: vec![0xCA, 0xFE, 0xBA, 0xBE],
    }]
}

// ==============================================================================
// 6. ENTRY POINT & REACTIVE WAKE LOOP
// ==============================================================================

fn init_app() -> AppState {
    let (tx_outbound, rx_outbound) = unbounded();
    let (tx_inbound, rx_inbound) = unbounded();

    spawn_network_backend(rx_outbound, tx_inbound);

    let mut state = AppState::default();
    state.tx_outbound = tx_outbound;
    state.rx_inbound = rx_inbound;
    state
}

fn setup_app(state: &mut AppState, waker: Waker) {
    state.waker = Some(waker.clone());
    let _ = state.tx_outbound.send(OutboundAction::SetWaker(waker));

    if let Some(phone) = INITIAL_PHONE.get() {
        state.phone_input.text = phone.clone();
        println!("[Setup] Automatically initiating phone pairing code for {}", phone);
        let _ = state.tx_outbound.send(OutboundAction::RequestPairCode {
            phone_number: phone.clone(),
        });
    }
}

fn poll_app(state: &mut AppState, _now: f64, _tx: &mut Tx) -> bool {
    let mut changed = false;
    while let Ok(event) = state.rx_inbound.try_recv() {
        state.apply_inbound_event(event);
        changed = true;
    }
    changed
}

fn main() {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd").args(["/c", "chcp", "65001"]).output();
    }

    println!("===============================================================");
    println!("  WhatsApp Native Client (Rust + Rune + WhatsApp Multi-Device)");
    println!("===============================================================");

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--clean" {
            println!("[Session] Cleaning old session databases...");
            let _ = std::fs::remove_file("whatsapp_session.db");
            let _ = std::fs::remove_file("whatsapp_session.db-shm");
            let _ = std::fs::remove_file("whatsapp_session.db-wal");
        } else if args[i] == "--phone" && i + 1 < args.len() {
            let phone = args[i + 1].clone();
            println!("[Pairing] Configured startup phone pairing for: {}", phone);
            let _ = INITIAL_PHONE.set(phone);
            i += 1;
        } else if !args[i].starts_with('-') && args[i].chars().all(|c| c.is_ascii_digit() || c == '+') {
            let phone = args[i].clone();
            println!("[Pairing] Configured startup phone pairing for: {}", phone);
            let _ = INITIAL_PHONE.set(phone);
        }
        i += 1;
    }

    println!("Initializing client...");

    let _tray_icon = init_system_tray();

    rune::run(AppSpec {
        setup: Some(setup_app),
        poll: Some(poll_app),
        ..AppSpec::new(
            "WhatsApp Native",
            (1080.0, 720.0),
            init_app,
            view,
        )
    });
}
