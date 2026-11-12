# WhatsApp Native Client (Rust + Rune)

An ultra-lightweight, high-performance standalone Windows desktop client for WhatsApp, implemented completely in pure Rust using the **Rune Native UI framework** (`github.com/anomalyco/rune`).

Designed for systems engineers and users requiring extreme efficiency:
* **Target Memory Footprint**: < 50MB RAM at runtime.
* **CPU Efficiency**: Near-zero idle CPU cycles via event-driven rendering.
* **Zero Web Technologies**: No Electron, WebView, Chromium, CEF, HTML, or CSS.

---

## Architecture Overview

```
                         +-----------------------------+
                         |      Windows Display        |
                         +-----------------------------+
                                        ▲
                                        │  GPU Frame Buffer (wgpu / DirectX 12)
                         +-----------------------------+
                         |   Rune Declarative UI Tree   |
                         |  (Sidebar, Bubbles, Status) |
                         +-----------------------------+
                                  ▲           │
                  Inbound Events  │           │ Outbound Actions
                 (Crossbeam Chan) │           ▼ (Crossbeam Chan)
                         +-----------------------------+
                         |   Background Actor Thread   |
                         +-----------------------------+
                           │            │            │
            WebSocket /    │            │            │  Windows OS
            Noise Protocol │            │ Media Save │  APIs
                           ▼            ▼            ▼
                     Meta Servers   Downloads   Tray / Toast
```

### Core Components

1. **Rune GPU UI Layer**
   - Declarative fluent builder syntax (`row()`, `col()`, `el()`, `text()`).
   - WhatsApp Dark Mode design system with responsive layouts.
   - Separate view pipelines for active conversations and status stories.

2. **Backend Protocol Engine**
   - Asynchronous network actor running on a dedicated low-overhead thread.
   - Built to interface with WhatsApp Multi-Device protocol stanzas over WebSockets.
   - End-to-end transport encryption utilizing Noise Protocol (`Noise_XX_25519_AESGCM_SHA256`).

3. **Windows Native Integrations**
   - **System Tray**: Minimizes to the Windows taskbar notification area via Win32 Shell API (`tray-icon`).
   - **Toast Notifications**: Native desktop notifications through Windows Notification Service (`notify-rust`).
   - **Status Media Downloader**: Dedicated download flow that fetches status media, decrypts the payload, and saves it directly to the user's Windows `Downloads` folder.

4. **WebRTC Voice & Video Foundation**
   - Low-latency hardware microphone capture via Windows Audio Session API (WASAPI) with `cpal`.
   - WebRTC session establishment, ICE candidate gathering, and DTLS-SRTP media encryption via pure Rust `webrtc-rs`.

---

## Project Structure

```
├── .gitignore          # Git exclusion rules for Rust and Windows builds
├── Cargo.toml          # Dependencies and aggressive size/performance release flags
├── LICENSE             # MIT License
├── README.md           # Documentation and build guide
└── src/
    └── main.rs         # Rune UI views, state management, and backend actors
```

---

## Getting Started

### Prerequisites

* **Operating System**: Windows 10/11 (or cross-compilation environment targeting `x86_64-pc-windows-msvc`).
* **Rust Toolchain**: Stable Rust 1.80+ (2021 edition).

```powershell
# Set default toolchain
rustup default stable-x86_64-pc-windows-msvc
```

### Development Build

```powershell
# Run the application in debug mode
cargo run
```

### Production Release Build (< 50MB Target)

The project includes an optimized release profile in `Cargo.toml`:
* `opt-level = "z"` (Aggressive size reduction)
* `lto = "fat"` (Full-program link-time optimization)
* `codegen-units = 1` (Maximum compiler optimization pass)
* `panic = "abort"` (Removes unwinding overhead)
* `strip = "symbols"` (Strips symbol table from final executable)

```powershell
# Compile the optimized executable
cargo build --release
```

The resulting standalone binary is located at:
`target\release\whatsapp-native-rune.exe`

---

## Status Updates & Media Saving

When viewing a contact's status in the Status tab:
1. The media preview and caption are rendered natively.
2. Clicking **"⬇ Download Status Media"** dispatches an async task to the backend actor.
3. Media is fetched, decrypted, saved to `C:\Users\<User>\Downloads`, and accompanied by a native Windows notification toast.

---

## License

This project is licensed under the [MIT License](LICENSE).
