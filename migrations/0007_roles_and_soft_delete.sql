-- Add role and soft-delete columns
ALTER TABLE users 
    ADD COLUMN role text not null default 'user',
    ADD COLUMN deleted_at timestamptz;

-- Add partial index for admin queries later
CREATE INDEX idx_users_role ON users (role) WHERE role = 'admin';
CREATE INDEX idx_users_deleted_at ON users (deleted_at) WHERE deleted_at IS NOT NULL;

-- Add CHECK constraint
ALTER TABLE users ADD CONSTRAINT chk_users_role CHECK (role IN ('user', 'admin'));
