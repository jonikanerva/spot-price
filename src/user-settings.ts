import type Database from "better-sqlite3";
import type { UserSettings } from "./types.js";

interface UserSettingsRow {
  user_id: string;
  margin_cents_kwh: number;
  transfer_day_cents_kwh: number;
  transfer_night_cents_kwh: number;
  tax_cents_kwh: number;
  vat_percent: number;
  night_start_hour: number;
  night_end_hour: number;
  timezone: string;
}

const rowToSettings = (row: UserSettingsRow): UserSettings => ({
  userId: row.user_id,
  marginCentsKwh: row.margin_cents_kwh,
  transferDayCentsKwh: row.transfer_day_cents_kwh,
  transferNightCentsKwh: row.transfer_night_cents_kwh,
  taxCentsKwh: row.tax_cents_kwh,
  vatPercent: row.vat_percent,
  nightStartHour: row.night_start_hour,
  nightEndHour: row.night_end_hour,
  timezone: row.timezone,
});

/** Get settings for a user, or null if not configured */
export const getUserSettings = (
  db: Database.Database,
  userId: string,
): UserSettings | null => {
  const row = db
    .prepare(`SELECT * FROM user_settings WHERE user_id = ?`)
    .get(userId) as UserSettingsRow | undefined;

  if (!row) {
    return null;
  }

  return rowToSettings(row);
};

/** Create or update user settings */
export const upsertUserSettings = (
  db: Database.Database,
  settings: UserSettings,
): void => {
  db.prepare(
    `INSERT OR REPLACE INTO user_settings
     (user_id, margin_cents_kwh, transfer_day_cents_kwh, transfer_night_cents_kwh,
      tax_cents_kwh, vat_percent, night_start_hour, night_end_hour, timezone, updated_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))`,
  ).run(
    settings.userId,
    settings.marginCentsKwh,
    settings.transferDayCentsKwh,
    settings.transferNightCentsKwh,
    settings.taxCentsKwh,
    settings.vatPercent,
    settings.nightStartHour,
    settings.nightEndHour,
    settings.timezone,
  );
};

/** Default settings for new Finnish spot-price users */
const DEFAULT_SETTINGS: Omit<UserSettings, "userId"> = {
  marginCentsKwh: 0.49,
  transferDayCentsKwh: 2.92,
  transferNightCentsKwh: 1.37,
  taxCentsKwh: 2.82752,
  vatPercent: 25.5,
  nightStartHour: 22,
  nightEndHour: 7,
  timezone: "Europe/Helsinki",
};

/** Ensure a user has default settings (create if missing) */
export const ensureUserSettings = (
  db: Database.Database,
  userId: string,
): UserSettings => {
  const existing = getUserSettings(db, userId);
  if (existing) {
    return existing;
  }

  const settings: UserSettings = { userId, ...DEFAULT_SETTINGS };
  upsertUserSettings(db, settings);

  const created = getUserSettings(db, userId);
  if (!created) {
    throw new Error("Failed to create default user settings");
  }
  return created;
};
