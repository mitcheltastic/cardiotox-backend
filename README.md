<div align="center">
  <h1>🫀 Cardiotox Backend</h1>
  <p>
    <strong>A high-performance, async Rust backend for the Cardiotox prediction platform.</strong>
  </p>
  <p>
    <img src="https://img.shields.io/badge/rust-%23000000.svg?style=for-the-badge&logo=rust&logoColor=white" alt="Rust" />
    <img src="https://img.shields.io/badge/Axum-000000?style=for-the-badge" alt="Axum" />
    <img src="https://img.shields.io/badge/postgresql-4169e1?style=for-the-badge&logo=postgresql&logoColor=white" alt="PostgreSQL" />
    <img src="https://img.shields.io/badge/docker-%230db7ed.svg?style=for-the-badge&logo=docker&logoColor=white" alt="Docker" />
  </p>
</div>

## 📖 Overview

This repository contains the backend service for the Cardiotox platform, built with [Rust](https://www.rust-lang.org/) and the [Axum](https://github.com/tokio-rs/axum) framework. 

It acts as a secure intermediary between the frontend application and the machine learning model hosted on a Hugging Face Space. It manages user authentication, proxies inference requests, and maintains an auditable history of predictions and SHAP explanations in a PostgreSQL database.

## ✨ Core Features

- **🔐 Authentication**
  - Passwordless Magic Link email authentication (via Resend).
  - Google OAuth integration.
  - Secure session management via `axum-login` and PostgreSQL session store.
- **🤖 ML Inference Proxy**
  - Seamlessly proxies `/predict` and `/explain` endpoints to a Gradio-based Hugging Face Space.
  - Normalizes inference outputs and formats them for the frontend.
- **📊 Audit & Telemetry**
  - Automatically logs user predictions (`prediction_logs`) and detailed SHAP feature contributions (`shap_logs`).
  - Structured application logging via `tracing` with `x-request-id` propagation.
- **🛡️ Security & Performance**
  - Configurable, multi-origin CORS support.
  - Rate limiting via `tower-governor` to prevent abuse.
  - Secure, `SameSite=None` cookie support for cross-domain deployments.

## 🚀 Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) (edition 2024 compatible)
- [PostgreSQL](https://www.postgresql.org/) (or a cloud provider like Neon)
- [Docker](https://www.docker.com/) (optional, for deployment)

### 1. Environment Setup

Copy the example environment file and configure your local variables:

```bash
cp .env.example .env
```

Ensure your `.env` is populated with the correct credentials, including your `DATABASE_URL`, Google OAuth keys, and Resend SMTP credentials.

### 2. Database Migrations

This project uses `sqlx` for database migrations. The application is configured to run migrations automatically on boot (`sqlx::migrate!`), but you can also run them manually using the `sqlx-cli`:

```bash
cargo install sqlx-cli
sqlx migrate run
```

### 3. Running Locally

Run the server in development mode:

```bash
cargo run
```

The server will bind to `0.0.0.0:3000` (or the `PORT` specified in your `.env`).

## 🐳 Deployment

This project includes a multi-stage `Dockerfile` optimized for minimal image size and fast builds, making it perfect for platforms like [Render](https://render.com) or [Fly.io](https://fly.io).

### Building the Image

```bash
docker build -t cardiotox-backend .
```

### Production Environment Variables

When deploying to production, pay special attention to the following environment variables:

- `COOKIE_SAMESITE=none` (Required for cross-site cookie usage)
- `COOKIE_SECURE=true` (Required when `SameSite=none`)
- `FRONTEND_ORIGIN` (Can be a comma-separated list of allowed origins, e.g., `https://app.example.com,https://staging.example.com`)
- `APP_BASE_URL` & `FRONTEND_URL` (Must point to the live domains)

## 📁 Project Structure

```text
├── migrations/          # SQLx database migrations
├── src/
│   ├── api.rs           # Core API handlers (Predict/Explain proxying)
│   ├── auth/            # Authentication logic (Email & Google OAuth)
│   ├── config.rs        # Environment configuration parsing
│   ├── db.rs            # Database connection & migration setup
│   ├── email.rs         # SMTP mailer implementation
│   ├── error.rs         # Global application error handling
│   ├── logging/         # Domain-specific logging
│   ├── models/          # Database models (User, ShapLog, etc.)
│   ├── services/        # External service integrations (Gradio Client)
│   ├── state.rs         # Axum application state
│   ├── telemetry.rs     # Tracing and observability setup
│   └── main.rs          # Application entrypoint & middleware assembly
├── Dockerfile           # Multi-stage production build configuration
└── Cargo.toml           # Rust dependencies and metadata
```

## 📜 License

This project is licensed under the MIT License.
