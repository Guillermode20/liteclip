# Cloud Subscription Plan — LiteClip Replay

## Overview

Keep `liteclip-core` (the recording engine) fully open source. Add a paid cloud sync feature that uploads saved clips to the cloud. The cloud feature lives in a new `src/cloud/` module in the binary crate and a separate Go backend.

**Stack:** Rust app (existing) + Go API server + PostgreSQL + S3 + Stripe

---

## Part 1: Go Backend

### 1.1 Project Structure

Create a new repo (e.g. `liteclip-cloud`):

```
liteclip-cloud/
├── cmd/server/main.go          # Entry point, router setup
├── internal/
│   ├── auth/
│   │   ├── handler.go          # POST /auth/signup, /auth/login, /auth/refresh
│   │   ├── jwt.go              # JWT generation/validation (HS256, 15min access + 7d refresh)
│   │   ├── middleware.go       # Auth middleware, extracts user_id from JWT
│   │   └── password.go         # bcrypt hash/verify
│   ├── clips/
│   │   ├── handler.go          # POST /clips/presign, GET /clips, DELETE /clips/:id
│   │   └── service.go          # Business logic, S3 presigned URL generation
│   ├── subscription/
│   │   ├── handler.go          # POST /subscription/checkout, POST /webhooks/stripe
│   │   └── stripe.go           # Stripe SDK calls, webhook signature verification
│   ├── user/
│   │   ├── repository.go       # User CRUD against PostgreSQL
│   │   └── model.go            # User struct
│   ├── clipmeta/
│   │   ├── repository.go       # Clip metadata CRUD
│   │   └── model.go            # Clip struct
│   ├── db/
│   │   ├── db.go               # PostgreSQL connection (pgxpool)
│   │   └── migrations/         # SQL migration files
│   │       ├── 001_init.up.sql
│   │       └── 001_init.down.sql
│   └── config/
│       └── config.go           # Env var loading (DATABASE_URL, S3_BUCKET, STRIPE_KEY, JWT_SECRET)
├── go.mod
├── go.sum
├── Dockerfile
└── .env.example
```

### 1.2 Database Schema

```sql
-- 001_init.up.sql
CREATE TABLE users (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email         TEXT UNIQUE NOT NULL,
    password_hash TEXT NOT NULL,
    stripe_customer_id TEXT,
    subscription_status TEXT NOT NULL DEFAULT 'inactive', -- inactive, active, past_due, canceled
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE clips (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    s3_key        TEXT NOT NULL,
    filename      TEXT NOT NULL,
    file_size     BIGINT NOT NULL,
    duration_secs REAL,
    game_name     TEXT,
    thumbnail_url TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at    TIMESTAMPTZ -- soft delete
);

CREATE INDEX idx_clips_user_id ON clips(user_id);
CREATE INDEX idx_clips_created_at ON clips(created_at);

CREATE TABLE subscriptions (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id              UUID UNIQUE NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    stripe_subscription_id TEXT UNIQUE NOT NULL,
    status               TEXT NOT NULL,
    current_period_end   TIMESTAMPTZ,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

### 1.3 API Endpoints

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| POST | `/auth/signup` | No | Create account (email, password). Returns JWT pair. |
| POST | `/auth/login` | No | Login. Returns JWT pair. |
| POST | `/auth/refresh` | No | Refresh access token. Returns new JWT pair. |
| GET | `/auth/profile` | Yes | Get current user info + subscription status. |
| POST | `/clips/presign` | Yes | Get S3 presigned PUT URL + clip metadata record. |
| POST | `/clips/confirm` | Yes | Confirm upload complete (update file_size, etc). |
| GET | `/clips` | Yes | List user's clips (paginated). |
| GET | `/clips/:id` | Yes | Get clip metadata + presigned GET URL. |
| DELETE | `/clips/:id` | Yes | Soft-delete clip + delete from S3. |
| POST | `/subscription/checkout` | Yes | Create Stripe Checkout Session. Returns URL. |
| POST | `/webhooks/stripe` | No | Stripe webhook. Updates subscription status. |
| GET | `/subscription/status` | Yes | Get current subscription details. |

### 1.4 Upload Flow (Presigned URL Pattern)

This avoids proxying large video files through the Go server:

1. Rust app calls `POST /clips/presign` with filename, duration, game_name
2. Go server checks subscription is active, generates S3 presigned PUT URL
3. Go server creates a `clips` record with `s3_key` but `file_size = 0`
4. Rust app uploads MP4 directly to S3 using the presigned URL
5. Rust app calls `POST /clips/confirm` with the clip ID
6. Go server updates `file_size` and marks upload as complete

### 1.5 Stripe Integration

- **Product:** "LiteClip Cloud" — single tier, $X/month or $Y/year
- **Checkout:** Server creates a Stripe Checkout Session, returns URL. User opens in browser.
- **Webhooks:** Listen for `checkout.session.completed`, `customer.subscription.updated`, `customer.subscription.deleted`. Update `users.subscription_status` accordingly.
- **Gating:** Every clip operation checks `subscription_status = 'active'`.

### 1.6 Tech Choices

- **Router:** `chi` (lightweight, idiomatic)
- **DB:** `pgx/v5` (best PostgreSQL driver for Go)
- **Migrations:** `golang-migrate/migrate`
- **Auth:** `golang-jwt/jwt/v5` + `golang.org/x/crypto/bcrypt`
- **S3:** `aws-sdk-go-v2/service/s3` (presigned URLs)
- **Stripe:** `stripe-go`
- **Validation:** `go-playground/validator`

---

## Part 2: Rust App Changes

### 2.1 New Module: `src/cloud/`

```
src/cloud/
├── mod.rs          # Public API: CloudClient, CloudConfig re-export
├── client.rs       # HTTP client (reqwest) wrapping Go API calls
├── upload.rs       # Clip upload orchestration (presign → S3 PUT → confirm)
├── auth.rs         # Token storage, refresh logic
└── types.rs        # Request/response types matching Go API
```

### 2.2 Dependency Addition

Add to `Cargo.toml` (binary crate only, NOT in `liteclip-core`):

```toml
[dependencies]
reqwest = { version = "0.12", features = ["json", "multipart", "stream"] }
# cloud feature is always compiled in the binary but gated at runtime by config
```

### 2.3 Config Changes (`crates/liteclip-core/src/config/config_mod/types.rs`)

Add a new `CloudConfig` section. This goes in core since config handling is there, but it's just a data struct — no cloud logic in core:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudConfig {
    /// Whether cloud sync is enabled (requires valid subscription)
    #[serde(default = "default_false")]
    pub enabled: bool,

    /// API base URL (default: production server)
    #[serde(default = "default_cloud_api_url")]
    pub api_url: String,

    /// Stored auth token (opaque, not user-visible)
    #[serde(default)]
    pub auth_token: String,

    /// Stored refresh token
    #[serde(default)]
    pub refresh_token: String,

    /// Whether to auto-upload clips after saving
    #[serde(default = "default_true")]
    pub auto_upload: bool,
}
```

Add `#[serde(default)] pub cloud: CloudConfig` to the `Config` struct.

### 2.4 Settings GUI Changes (`src/gui/settings.rs`)

Add a `Cloud` tab:

1. Add `Cloud` variant to `SettingsTab` enum
2. Add `render_cloud_settings(ui)` method showing:
   - **Logged out state:** Email/password fields, "Sign Up" and "Log In" buttons
   - **Logged in state:** Email display, subscription status badge, "Auto-upload clips" toggle, "Log Out" button
   - **Subscription management:** "Manage Subscription" button (opens Stripe billing portal URL in browser)
3. Wire into existing tab system (add to `all()`, `label()`, `match`)

### 2.5 Clip Save Integration (`src/main.rs`)

Hook into `spawn_save_clip_task`. After the existing `ClipManager::save_clip()` succeeds:

```rust
// In spawn_save_clip_task, after the existing save logic:
let cloud_client = cloud_client_clone; // Arc<CloudClient>
if config.cloud.enabled && config.cloud.auto_upload {
    let clip_path = final_path.clone();
    tokio::spawn(async move {
        match cloud_client.upload_clip(&clip_path).await {
            Ok(clip_id) => {
                info!("Clip uploaded to cloud: {}", clip_id);
                // optional: show toast
            }
            Err(e) => {
                warn!("Cloud upload failed: {}", e);
                // non-fatal: clip is still saved locally
            }
        }
    });
}
```

The upload is fire-and-forget (non-blocking, non-fatal on failure). The clip is always saved locally first.

### 2.6 Cloud Client (`src/cloud/client.rs`)

```rust
pub struct CloudClient {
    base_url: String,
    http: reqwest::Client,
    auth_token: RwLock<Option<String>>,
    refresh_token: RwLock<Option<String>>,
}

impl CloudClient {
    pub async fn signup(&self, email: &str, password: &str) -> Result<AuthResponse>;
    pub async fn login(&self, email: &str, password: &str) -> Result<AuthResponse>;
    pub async fn refresh(&self) -> Result<AuthResponse>;
    pub async fn profile(&self) -> Result<UserProfile>;
    pub async fn upload_clip(&self, path: &Path) -> Result<String>;
    pub async fn list_clips(&self, page: i32) -> Result<ClipListResponse>;
    pub async fn delete_clip(&self, clip_id: &str) -> Result<()>;
    pub async fn create_checkout_session(&self) -> Result<String>;
}
```

### 2.7 Token Storage

JWT tokens are stored in `CloudConfig` (which lives in the TOML config file at `%APPDATA%\liteclip-replay\liteclip-replay.toml`). The file is user-local and permissions-restricted by Windows ACLs. For a desktop app, this is acceptable — there's no server to steal tokens from.

Access tokens are short-lived (15 min). The client auto-refreshes using the refresh token before API calls.

### 2.8 Gallery Integration (Future, Not MVP)

A future enhancement could add a "Cloud" view in the gallery that fetches clip thumbnails/metadata from the API and allows downloading clips from cloud storage. This is out of scope for the initial implementation.

---

## Part 3: Feature Gating Strategy

### Runtime Gating (Not Compile-Time)

The cloud feature is **always compiled** into the binary but **gated at runtime** by `config.cloud.enabled`. This means:

- The `src/cloud/` module is always compiled
- `reqwest` is always a dependency
- But no cloud code executes unless `config.cloud.enabled = true` AND the user has an active subscription
- If `config.cloud.enabled = false`, the Cloud tab still shows but displays a "Sign up to enable cloud sync" prompt

This approach is simpler than compile-time feature flags and avoids maintaining two build variants.

### Open Source Clarity

The open source repo includes the cloud module code. The Go backend is a separate private repo. Users can:
- Use the app fully offline without any cloud features
- Self-host the Go backend if they want (the API protocol is documented)
- The cloud module is ~500 lines of straightforward HTTP client code — nothing proprietary

---

## Part 4: Implementation Order

### Phase 1: Go Backend (standalone)
1. Project scaffold (go.mod, Dockerfile, .env)
2. Database schema + migrations
3. Auth endpoints (signup, login, refresh, profile)
4. Clip endpoints (presign, confirm, list, get, delete)
5. S3 integration (presigned URL generation)
6. Stripe integration (checkout, webhooks, subscription status)
7. Deploy (e.g., Fly.io, Railway, or a VPS)

### Phase 2: Rust App — Config & Settings
1. Add `CloudConfig` to `types.rs`
2. Add `Cloud` tab to settings GUI
3. Add login/signup UI in the Cloud tab
4. Test config round-trip (save/load TOML)

### Phase 3: Rust App — Upload Pipeline
1. Create `src/cloud/` module with `CloudClient`
2. Implement auth flow (signup, login, token refresh)
3. Implement clip upload (presign → S3 PUT → confirm)
4. Hook into `spawn_save_clip_task` in `main.rs`
5. Add upload status toast notifications

### Phase 4: Polish
1. Error handling & retry logic for uploads
2. Upload queue (if multiple clips saved rapidly)
3. Storage usage display in Cloud settings tab
4. "Manage Subscription" button (Stripe billing portal)
5. End-to-end testing

---

## Part 5: Key Files to Modify

| File | Change |
|------|--------|
| `Cargo.toml` (root) | Add `reqwest` dependency |
| `crates/liteclip-core/src/config/config_mod/types.rs` | Add `CloudConfig` struct + field on `Config` |
| `src/gui/settings.rs` | Add `Cloud` tab + `render_cloud_settings()` |
| `src/main.rs` | Hook cloud upload after clip save |
| `src/lib.rs` | Add `pub mod cloud;` |

New files:
| File | Purpose |
|------|---------|
| `src/cloud/mod.rs` | Module facade |
| `src/cloud/client.rs` | HTTP client wrapping Go API |
| `src/cloud/upload.rs` | Upload orchestration |
| `src/cloud/auth.rs` | Token management |
| `src/cloud/types.rs` | API request/response types |
