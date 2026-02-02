# Lightspeed

> **High-Performance Local File Transfer**

Lightspeed is a blazingly fast, zero-configuration file transfer tool designed for local networks. Built with Rust, it leverages **QUIC** (via `quinn`) for reliable, congested-controlled parallel transport and **Memory Mapping** for efficient I/O.

## Features

- 🚀 **High Speed**: Utilizes QUIC parallel streams and zero-copy `sendfile`/`pwrite` optimizations (Linux).
  > **Note**: The QUIC implementation is currently experimental and a work-in-progress (TODO).
- 🖥️ **TUI Interface**: Simple, interactive Terminal User Interface built with `ratatui`.
- 🔍 **Zero-Config Discovery**: Automatically finds peers on the local LAN using UDP Multicast.
- 🔒 **Secure-ish**: Uses TLS 1.3 (Self-signed) for encryption during transit.
- 📦 **Start & Forget**: Just run the binary on two machines; they'll find each other.

## Installation

### Prerequisites
- **Rust Toolchain**: [Install Rust](https://rustup.rs/)
- **Linux**: Recommended for best performance (uses `pwrite` optimizations).

### Build from Source
```bash
git clone https://github.com/yourusername/lightspeed.git
cd lightspeed
cargo build --release
```

The binary will be located at `target/release/lightspeed`.

## Usage

1. **Start the application** on both the sender and receiver machines:
   ```bash
   ./target/release/lightspeed
   ```
   
2. **The TUI will launch**:
   - You will see a list of discovered peers on the left.
   - Use `Up/Down` arrows to select a peer.
   - Press `Enter` to select a peer and open the file picker.
   - Select a file to send.
   
3. **Receiver**:
   - The receiver will automatically accept incoming transfers.
   - Files are saved to the current working directory, renamed with a `quic_rec_` prefix (e.g., `quic_rec_filename.ext`).

## Technical Details

- **Protocol**: QUIC (over single UDP port).
- **Discovery**: UDP Multicast on `239.255.0.1:9999`.
- **Concurrency**: Breaks files into chunks and sends them over up to 200 parallel QUIC streams.
- **Integrity**: BLAKE3 hashing is performed on every chunk to ensure data corruption is detected.

## Disclaimer

> [!WARNING]
> This project is currently in **Alpha**.
> - It uses self-signed certificates without manual verification (TOFU is not fully implemented), making it vulnerable to active Man-in-the-Middle attacks on untrusted networks.
> - **QUIC Implementation**: The QUIC protocol support is currently a prototype/TODO and may be unstable.
> - **Use only on trusted local networks.**
