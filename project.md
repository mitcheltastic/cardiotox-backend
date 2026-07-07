# Simple Axum Backend

A minimal, best-practice Rust backend server utilizing the **Axum** web framework, **Tokio** asynchronous runtime, and **Tracing** for diagnostic logging.

## Tech Stack

- **Language:** Rust (Edition 2024)
- **Web Framework:** [Axum (v0.8)](https://github.com/tokio-rs/axum)
- **Async Runtime:** [Tokio (v1)](https://github.com/tokio-rs/tokio) with `full` features
- **Logging & Diagnostics:** [Tracing (v0.1)](https://github.com/tokio-rs/tracing) & [Tracing Subscriber (v0.3)](https://github.com/tokio-rs/tracing)

## Project Structure

```text
.
├── Cargo.toml          # Rust package manifest
├── Cargo.lock          # Dependency lockfile
├── .gitignore          # Git ignore rules
└── src
    └── main.rs         # Server implementation, endpoints, and graceful shutdown
```

## Features & Implementation Details

### 1. Endpoints
The server registers a single endpoint:
- **`GET /`**
  - **Handler:** `root()`
  - **Response:** Returns the plain-text message `"Hilmy is Gay"`.
  - **Status Code:** `200 OK`

### 2. Structured Logging
The project uses `tracing-subscriber` to format and print logs to stdout.
- The logging verbosity is configurable via the `RUST_LOG` environment variable.
- Defaults to `info` level logging.

### 3. Graceful Shutdown
The server includes a graceful shutdown mechanism triggered by a `Ctrl+C` (SIGINT) signal.
- The `shutdown_signal()` helper listens for `ctrl_c()`.
- Upon receiving the signal, it logs the event and exits the listener, allowing the server to drain any in-flight connections before shutting down completely.

---

## Getting Started

### Prerequisites
Make sure you have Rust and Cargo installed:
```bash
cargo --version
```

### Running the Server
To run the server locally:
```bash
cargo run
```
The server will start listening on `http://0.0.0.0:3000`.

### Testing the Endpoint
To verify the endpoint, you can send an HTTP request using `curl`:
```bash
curl http://localhost:3000/
```
Expected output:
```text
Hilmy is Gay
```
