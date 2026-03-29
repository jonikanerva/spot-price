import type { Pool } from "pg";
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
  area: string;
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
  area: row.area,
});

/** Get settings for a user, or null if not configured */
export const getUserSettings = async (
  pool: Pool,
  userId: string,
): Promise<UserSettings | null> => {
  const { rows } = await pool.query<UserSettingsRow>(
    `SELECT * FROM user_settings WHERE user_id = $1`,
    [userId],
  );

  const row = rows[0];
  return row ? rowToSettings(row) : null;
};

/** Create or update user settings */
export const upsertUserSettings = async (
  pool: Pool,
  settings: UserSettings,
): Promise<void> => {
  await pool.query(
    `INSERT INTO user_settings
     (user_id, margin_cents_kwh, transfer_day_cents_kwh, transfer_night_cents_kwh,
      tax_cents_kwh, vat_percent, night_start_hour, night_end_hour, timezone, area, updated_at)
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
     ON CONFLICT (user_id)
     DO UPDATE SET margin_cents_kwh = EXCLUDED.margin_cents_kwh,
                   transfer_day_cents_kwh = EXCLUDED.transfer_day_cents_kwh,
                   transfer_night_cents_kwh = EXCLUDED.transfer_night_cents_kwh,
                   tax_cents_kwh = EXCLUDED.tax_cents_kwh,
                   vat_percent = EXCLUDED.vat_percent,
                   night_start_hour = EXCLUDED.night_start_hour,
                   night_end_hour = EXCLUDED.night_end_hour,
                   timezone = EXCLUDED.timezone,
                   area = EXCLUDED.area,
                   updated_at = EXCLUDED.updated_at`,
    [
      settings.userId,
      settings.marginCentsKwh,
      settings.transferDayCentsKwh,
      settings.transferNightCentsKwh,
      settings.taxCentsKwh,
      settings.vatPercent,
      settings.nightStartHour,
      settings.nightEndHour,
      settings.timezone,
      settings.area,
      new Date().toISOString(),
    ],
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
  area: "FI",
};

/** Ensure a user has default settings (create if missing) */
export const ensureUserSettings = async (
  pool: Pool,
  userId: string,
): Promise<UserSettings> => {
  const existing = await getUserSettings(pool, userId);
  if (existing) {
    return existing;
  }

  const settings: UserSettings = { userId, ...DEFAULT_SETTINGS };
  await upsertUserSettings(pool, settings);

  const created = await getUserSettings(pool, userId);
  if (!created) {
    throw new Error("Failed to create default user settings");
  }
  return created;
};
