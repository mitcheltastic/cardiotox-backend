CREATE TABLE email_tokens (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash text NOT NULL,
    kind text NOT NULL,
    expires_at timestamptz NOT NULL,
    used_at timestamptz
);

CREATE INDEX idx_email_tokens_token_hash ON email_tokens(token_hash);
