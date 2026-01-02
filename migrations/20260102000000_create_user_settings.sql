-- Create user_settings table
CREATE TABLE IF NOT EXISTS user_settings (
    user_id TEXT NOT NULL,
    guild_id TEXT NOT NULL,
    transcribe_name TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY(user_id, guild_id)
);

-- Create index for faster lookups by guild
CREATE INDEX IF NOT EXISTS idx_user_settings_guild ON user_settings(guild_id);

-- Create index for faster lookups by user and guild
CREATE INDEX IF NOT EXISTS idx_user_settings_user_guild ON user_settings(user_id, guild_id);

