# WhatsApp Native Client (Rust + Rune)

An ultra-lightweight, high-performance standalone Windows desktop client for WhatsApp, implemented completely in pure Rust using the **Rune Native UI framework** (`github.com/anomalyco/rune`) and real WhatsApp Multi-Device backend protocols.

> 📖 **Developer Notes & Status Log:** See [NOTES.md](NOTES.md) for full technical documentation of all bug fixes, architecture decisions, protocol handshake troubleshooting, and future roadmap.

Designed for systems engineers and users requiring extreme efficiency:
* **Target Memory Footprint**: < 35MB RAM at runtime (measured under live connection).
* **CPU Efficiency**: Near-zero idle CPU cycles via event-driven reactive wakeups.
* **Zero Web Technologies**: No Electron, WebView, Chromium, CEF, HTML, or CSS.
* **Native E2EE Messaging**: Full Noise-XX handshake + Signal Protocol Libsignal end-to-end encryption.
* **Dual Pairing System**: QR code camera scanning (Chrome on Windows) and 8-character phone number pairing code.

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
├── README.md           # Documentation and setup guide
└── src/
    └── main.rs         # Rune UI views, state management, and backend actors
```

---

## Getting Started

### 1. Prerequisites & Installing Requirements

Before compiling the application on Windows, verify that the following tools are installed:

#### A. Install Git
Download and install Git from [git-scm.com](https://git-scm.com/) or via `winget`:
```powershell
winget install --id Git.Git -e --source winget
```

#### B. Install Visual Studio C++ Build Tools
Rust on Windows requires the MSVC linker and C++ toolchain:
```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--passive --add Microsoft.VisualStudio.Workload.VCTools"
```

#### C. Install Rust (Rustup)
Download and run `rustup-init.exe` from [rustup.rs](https://rustup.rs/) or install via `winget`:
```powershell
winget install --id Rustlang.Rustup -e --source winget
```

Configure the default toolchain to 64-bit Windows MSVC:
```powershell
rustup default stable-x86_64-pc-windows-msvc
rustup update
```

---

### 2. Clone the Repository

Clone the repository and navigate into the project directory:

```powershell
git clone https://github.com/pthmn3/whatsapp-rust.git
cd whatsapp-rust
```

---

### 3. Build & Run

#### Development Mode (Fast Compile)
To compile and launch the application during development:
```powershell
cargo run
```

#### Production Release Build (< 50MB RAM Target)
To produce the ultra-optimized standalone release binary with whole-program Link-Time Optimization (Fat LTO) and stripped symbols:
```powershell
cargo build --release
```

The resulting standalone executable will be located at:
```text
target\release\whatsapp-native-rune.exe
```

---

## Live WhatsApp Multi-Device Pairing & Connectivity

The application connects directly to WhatsApp Multi-Device servers (`wss://web.whatsapp.com/ws/chat`) with Noise Protocol transport encryption and Signal Protocol end-to-end encryption.

### 1. Pairing with Your Account
When running the client for the first time:
- **Option A (QR Code Scanning)**: Point your phone camera at the QR code displayed on screen (or printed to your terminal) under **WhatsApp > Linked Devices > Link a Device**. The client identifies as Google Chrome on Windows (`PlatformType::CHROME`), which is natively accepted by WhatsApp servers.
- **Option B (Phone Number Linking)**: In the app window, enter your phone number with country code (e.g. `15551234567`) in the box under *"— OR LINK WITH PHONE NUMBER —"* and click **Get Code**. In WhatsApp on your phone, navigate to **Linked Devices > Link a Device > Link with phone number instead**, and type the 8-character code.
- **Command-Line Direct Pairing**: You can also launch the executable directly with your phone number:
  ```powershell
  .\whatsapp-native-rune.exe --phone 15551234567
  ```
- **Session Cleanup**: To reset a session or start fresh, pass the `--clean` flag:
  ```powershell
  .\whatsapp-native-rune.exe --clean
  ```
- **Session Persistence**: Session keys and device credentials are automatically stored in `whatsapp_session.db` (SQLite). Subsequent launches connect immediately without re-pairing.

### 2. Live Messaging
- **Interactive Composer**: Type directly into the bottom input field and press `Enter` or click `➤` to dispatch end-to-end encrypted messages to any contact or phone number.
- **Search & Start Chat**: Enter any international phone number (e.g. `+1234567890`) in the top search box and press `+` to initiate a conversation.
- **Native Notifications**: Incoming messages trigger real-time Windows toast notifications and append to the active chat bubble stream.

---

## Status Updates & Media Saving

When viewing a contact's status in the Status tab:
1. The media preview and caption are rendered natively.
2. Clicking **"⬇ Download Status Media"** dispatches an async task to the backend actor.
3. Media is fetched, decrypted, saved to `C:\Users\<User>\Downloads`, and accompanied by a native Windows notification toast.

---

## License

This project is licensed under the [MIT License](LICENSE).
