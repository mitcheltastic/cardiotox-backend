CREATE TABLE action_codes (
    id uuid primary key default gen_random_uuid(),
    user_id uuid not null references users(id) on delete cascade,
    code_hash text not null,
    action text not null,
    expires_at timestamptz not null,
    used_at timestamptz,
    attempts int not null default 0,
    created_at timestamptz not null default now()
);

CREATE INDEX idx_action_codes_user_action ON action_codes(user_id, action);
