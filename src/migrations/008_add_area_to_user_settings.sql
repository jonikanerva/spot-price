-- Add delivery area column to user settings (default FI for existing users)
ALTER TABLE user_settings ADD COLUMN area TEXT NOT NULL DEFAULT 'FI';
