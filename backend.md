# backend.md — Handoff for Rust/Axum Auth + Logging Backend

> Paste this into a new chat to start the build. Goal: a production-shaped Rust backend for my
> AI website providing (1) email auth, (2) Google OAuth, (3) a logging system. Built on Axum 0.8.

---

## 0. Working style (how we collaborate)

- I (**HCiM**) work in **Antigravity IDE** (Gemini-based agentic IDE). The assistant is a
  **prompt partner**: give me clean, copy-paste-ready prompts to feed the Antigravity agent,
  which executes them. Don't write big code blocks directly unless I hit a persistent bug.
- **One clear step at a time.** Decisive recommendations over option menus. Build in phases
  (see §9), checkpoint to disk each phase, and run `cargo check`/`cargo test` after each.
- Casual tone, English + Bahasa Indonesia mix is fine.
- I just learned basic Axum (a hello-world server with a handler, tracing, graceful shutdown).
  Assume I'm new to Rust web auth — explain the *why* briefly, then give the prompt.

---

## 1. What this backend is for (context)

My AI product is an **in-silico cardiotoxicity (TdP) risk screening** web app. The ML model is
already deployed on Hugging Face Spaces (Gradio), exposing `/predict` and `/explain`. The
frontend is a **Next.js app ("Cardiotox")**. This backend adds the things the HF Space can't:
**user accounts, login, and request/audit logging**. Optionally it also proxies prediction
calls to the HF Space so every prediction can be logged and tied to a user.

So the backend sits: **Next.js frontend → this Rust backend (auth + logging + proxy) → HF Space.**

---

## 2. Key architecture decisions (already made — treat as final)

1. **Session-based auth, not JWT.** Server-side sessions via `axum-login` + `tower-sessions`
   with a Postgres session store. Reason: it's a browser app, sessions give instant revocation
   (logout, ban), simpler CSRF story, and no token-refresh complexity. Cookie is HttpOnly +
   Secure + signed/encrypted.
2. **Database: PostgreSQL** via `sqlx` (async, compile-time-checked queries, migrations).
   *SQLite is an acceptable drop-in for pure local dev* (`sqlx` supports both) but Postgres is
   the target.
3. **Google OAuth via the raw `oauth2` crate (v5)** using **Authorization Code + PKCE + state**.
   Reason: transparent, no heavy framework magic, easy to defend/understand, integrates cleanly
   with our own session layer. Users are linked by verified email (account linking).
4. **Logging = two layers:** (a) **operational** — `tracing` + `tracing-subscriber` (JSON to
   stdout) + `tower-http` `TraceLayer` with a per-request ID; (b) **audit/analytics** — write
   auth events and prediction requests to DB tables.
5. **Hosting shape:** put frontend and backend on the **same parent domain**
   (`app.example.com` + `api.example.com`) so session cookies can use `SameSite=Lax` and stay
   simple/safe. (If they end up cross-site, cookies must be `SameSite=None; Secure` and CORS
   must allow credentials for the exact origin — note this at deploy time.)

---

## 3. Tech stack / crates

Let the Antigravity agent add these with `cargo add` so it resolves the latest compatible
patch versions (don't hardcode patch numbers — they drift). Major versions + features:

```
# web core
axum                = "0.8"                      # (features: macros if needed)
tokio               = "1"      full
tower               = "0.5"
tower-http          = "0.6"    ["trace","cors","request-id","util"]

# auth + sessions
axum-login          = "0.18"                     # brings tower-sessions
tower-sessions-sqlx-store = latest  ["postgres"] # Postgres-backed session store
argon2              = "0.5"                       # Argon2id password hashing
password-hash       = "0.5"

# oauth
oauth2              = "5"                         # Authorization Code + PKCE
reqwest             = "0.12"   ["json","rustls-tls"]   # token exchange + userinfo + HF proxy

# db
sqlx                = "0.8"    ["runtime-tokio-rustls","postgres","uuid","time","migrate"]

# email
lettre              = "0.11"   ["tokio1-rustls-tls","smtp-transport","builder"]

# util
serde / serde_json  = "1"      serde: ["derive"]
validator           = latest   ["derive"]         # email/password validation
thiserror           = "2"                          # typed errors
anyhow              = "1"
dotenvy             = "0.15"
uuid                = "1"       ["v4","serde"]
time                = "0.3"
rand                = latest                        # token generation

# logging
tracing             = "0.1"
tracing-subscriber  = "0.3"    ["env-filter","json"]
```

Edition 2024. seed nothing random-critical without `rand`'s CSPRNG.

---

## 4. Project structure (target)

```text
src/
  main.rs              # bootstrap: config, telemetry, db, router, graceful shutdown
  config.rs            # load + validate env into a Config struct
  error.rs             # AppError enum (thiserror) + IntoResponse impl
  state.rs             # AppState { db pool, oauth client, mailer, config } (Clone, Arc inside)
  telemetry.rs         # tracing-subscriber JSON + EnvFilter setup
  db.rs                # PgPool builder + run migrations on startup
  auth/
    mod.rs
    password.rs        # argon2id hash + verify
    backend.rs         # axum-login: AuthUser + AuthnBackend (Credentials = email/password)
    email_auth.rs      # register / login / logout / verify-email / password reset handlers
    google_oauth.rs    # /auth/google + /auth/google/callback (PKCE + state)
    tokens.rs          # email-verify + password-reset token create/validate (store HASH only)
  email/
    mod.rs             # lettre SMTP mailer + send_verification / send_reset
  models/
    user.rs  oauth_account.rs  prediction_log.rs  auth_event.rs
  routes.rs            # assemble Router, apply layers (session, trace, cors), mount protected
  logging/
    audit.rs           # write auth_events + prediction_logs rows
  services/
    prediction.rs      # reqwest client to the HF Space /predict + /explain (optional proxy)
migrations/            # sqlx migrations (0001_users.sql, ...)
.env / .env.example
```

---

## 5. Data model (Postgres)

```sql
-- users: password_hash is NULL for OAuth-only accounts
users(
  id             uuid primary key default gen_random_uuid(),
  email          text unique not null,
  email_verified boolean not null default false,
  password_hash  text,                     -- nullable (OAuth-only)
  display_name   text,
  created_at     timestamptz not null default now(),
  updated_at     timestamptz not null default now()
)

-- linked social logins (one user can have password + google)
oauth_accounts(
  id               uuid primary key default gen_random_uuid(),
  user_id          uuid not null references users(id) on delete cascade,
  provider         text not null,           -- 'google'
  provider_user_id text not null,           -- Google 'sub'
  created_at       timestamptz not null default now(),
  unique(provider, provider_user_id)
)

-- single-use tokens; store only a SHA-256 hash of the token, never the raw value
email_tokens(
  id         uuid primary key default gen_random_uuid(),
  user_id    uuid not null references users(id) on delete cascade,
  token_hash text not null,
  kind       text not null,                 -- 'verify' | 'reset'
  expires_at timestamptz not null,
  used_at    timestamptz
)

-- audit log for auth events (analytics + security)
auth_events(
  id         uuid primary key default gen_random_uuid(),
  user_id    uuid references users(id) on delete set null,
  event      text not null,                 -- 'register','login_ok','login_fail','logout','oauth_login','verify','reset'
  ip         inet,
  user_agent text,
  created_at timestamptz not null default now()
)

-- every prediction routed through the backend (ties model output to a user)
prediction_logs(
  id             uuid primary key default gen_random_uuid(),
  user_id        uuid references users(id) on delete set null,
  input          jsonb not null,            -- the 11 biomarker values
  predicted_tier text,                      -- 'high'|'intermediate'|'low'
  probabilities  jsonb,
  latency_ms     integer,
  created_at     timestamptz not null default now()
)

-- session store table is created/managed by tower-sessions-sqlx-store (migrate on boot)
```

---

## 6. Endpoints

**Email auth**
- `POST /auth/register` — {email, password, display_name} → create user (Argon2id hash), send verification email, 201. Validate email format + password strength.
- `GET  /auth/verify?token=…` — mark `email_verified=true` if token valid+unused+unexpired.
- `POST /auth/login` — {email, password} → verify hash, require `email_verified`, create session, set cookie.
- `POST /auth/logout` — destroy session.
- `POST /auth/password/forgot` — {email} → always 200 (don't leak existence), email a reset link if user exists.
- `POST /auth/password/reset` — {token, new_password} → set new hash, invalidate token + existing sessions.
- `GET  /auth/me` — **protected**, returns current user profile.

**Google OAuth**
- `GET  /auth/google` — build authorize URL with PKCE challenge + CSRF `state`, stash the PKCE verifier + state in the session, 302 redirect to Google.
- `GET  /auth/google/callback?code&state` — verify `state` matches session; exchange `code` (+ PKCE verifier) for tokens; call Google userinfo (`https://openidconnect.googleapis.com/v1/userinfo`); upsert `users` + `oauth_accounts` by `sub`/email; create session; redirect to frontend. Scopes: `openid email profile`.

**Prediction proxy (optional but recommended — ties auth + logging + the AI product)**
- `POST /api/predict` — **protected**; accept 11 biomarkers, forward to the HF Space, write a `prediction_logs` row (input, tier, probabilities, latency), return result.
- `POST /api/explain` — **protected**; same idea against the HF `/explain`.

**Health**
- `GET /healthz` — liveness (no auth).

---

## 7. Logging system (details)

- **Startup:** `tracing_subscriber` with `EnvFilter` (from `RUST_LOG`, default `info`) + JSON
  formatter → stdout (container/HF-friendly). Put this in `telemetry.rs`, call first in `main`.
- **Request tracing:** `tower_http::trace::TraceLayer` producing a span per request; add
  `tower_http::request_id` (`SetRequestIdLayer` + `PropagateRequestIdLayer`) so every log line
  carries an `x-request-id`. Record method, path, status, latency.
- **Auth audit:** on register/login/logout/oauth/verify/reset, write an `auth_events` row via
  `logging::audit`. Capture IP + user-agent from headers.
- **Prediction audit:** the `/api/predict` handler writes a `prediction_logs` row. This is
  gold for the thesis/product (usage analytics + reproducibility).
- **Never log secrets:** no passwords, tokens, cookies, or full Authorization headers.

---

## 8. Security checklist (bake in from the start)

- Passwords hashed with **Argon2id** (never store plaintext; never log).
- **Email + reset tokens:** generate a random 32-byte token, email the raw value, store only
  its SHA-256 hash; single-use; short TTL (verify ~24h, reset ~1h).
- **Session cookie:** HttpOnly, Secure, `SameSite=Lax` (Lax lets the cookie ride the top-level
  OAuth redirect back). Use a signed/encrypted session cookie; keep the signing key in env.
- **CSRF:** for cookie-session state-changing routes, add a double-submit CSRF token (or rely
  on SameSite=Lax + custom header check for same-site setups). Decide in Phase 4.
- **CORS:** `tower_http::cors` — allow *only* the exact frontend origin, `allow_credentials(true)`.
- **Rate limit** login/register/forgot (e.g. `tower_governor`) to blunt brute force. Phase 5.
- **OAuth:** validate `state`, use PKCE `S256`, verify Google's `sub`, only trust `email` if
  `email_verified` is true in the userinfo response.
- **Don't leak account existence** on login/forgot error messages.
- HTTPS only in production.

---

## 9. NEXT SESSION GOAL — phased build plan

Build in order; `cargo check` + a manual `curl` smoke test after each phase; checkpoint to disk.

- **Phase 0 — Skeleton:** config loader, `telemetry.rs` (JSON tracing), `PgPool` + migrate on
  boot, `AppState`, `error.rs`, `/healthz`, graceful shutdown. Server runs, logs JSON.
- **Phase 1 — Email auth core:** `users` table, Argon2id hash/verify, `axum-login` backend
  (`AuthUser` + `AuthnBackend`), Postgres session store, `register` / `login` / `logout` /
  `me`. Protect `/auth/me` with `login_required`.
- **Phase 2 — Email flows:** `lettre` mailer, `email_tokens` table, email verification +
  password forgot/reset. Gate login on `email_verified`.
- **Phase 3 — Google OAuth:** `oauth2` client, `/auth/google` + callback with PKCE + state,
  `oauth_accounts`, account linking by email, session on success.
- **Phase 4 — Logging system:** `TraceLayer` + request-id, `auth_events` audit writes,
  finalize CSRF + CORS decisions.
- **Phase 5 — Prediction proxy:** `services/prediction.rs` (reqwest → HF Space), `/api/predict`
  + `/api/explain`, `prediction_logs`, rate limiting. Wire the Next.js frontend to this backend.

---

## 10. Environment variables (.env.example)

```
DATABASE_URL=postgres://user:pass@localhost:5432/cardiotox
APP_BASE_URL=https://api.example.com          # this backend's public URL
FRONTEND_URL=https://app.example.com          # for CORS + OAuth success redirect
SESSION_SECRET=<64+ random bytes, base64>     # cookie signing/encryption key
GOOGLE_CLIENT_ID=<...>.apps.googleusercontent.com
GOOGLE_CLIENT_SECRET=<...>
GOOGLE_REDIRECT_URL=https://api.example.com/auth/google/callback
SMTP_HOST=smtp.example.com
SMTP_PORT=587
SMTP_USER=<...>
SMTP_PASS=<...>
EMAIL_FROM="Cardiotox <no-reply@example.com>"
HF_SPACE_BASE=https://mitcheltastic-tdp-cipa-screening.hf.space   # for the prediction proxy
RUST_LOG=info
```

Google setup: create an OAuth Client ID at console.cloud.google.com → Credentials, add the
exact `GOOGLE_REDIRECT_URL` as an authorized redirect URI, and add your domain to the OAuth
consent screen.

---

## 11. First prompt to run in the new chat

> "Read backend.md. We're on **Phase 0**. Give me a single copy-paste Antigravity prompt to
> scaffold the project: Cargo.toml with the crates in §3 (use `cargo add`), the `src/` module
> layout from §4 (empty stubs), `config.rs` loading the §10 env vars via dotenvy, `telemetry.rs`
> with JSON tracing + EnvFilter, `db.rs` building a PgPool and running migrations on boot,
> `AppState`, an `AppError` type, a `/healthz` route, and graceful shutdown. Then a curl smoke
> test. Stop after Phase 0 so I can verify before Phase 1."
