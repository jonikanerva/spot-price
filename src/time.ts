/** Get today and tomorrow date strings in the given timezone */
export const getCurrentAndNextDate = (
  timeZone: string,
): { today: string; tomorrow: string } => {
  const now = new Date();
  return {
    today: formatDateInTimeZone(now, timeZone),
    tomorrow: formatDateInTimeZone(addDays(now, 1), timeZone),
  };
};

/** Format a Date as YYYY-MM-DD using UTC components */
export const formatUtcDate = (date: Date): string => {
  const year = date.getUTCFullYear();
  const month = String(date.getUTCMonth() + 1).padStart(2, "0");
  const day = String(date.getUTCDate()).padStart(2, "0");
  return `${String(year)}-${month}-${day}`;
};

const getDatePart = (
  date: Date,
  timeZone: string,
  partType: "year" | "month" | "day",
): string => {
  const formatter = new Intl.DateTimeFormat("en-US", {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  });

  const parts = formatter.formatToParts(date);
  const value = parts.find((p) => p.type === partType)?.value;
  if (!value) {
    throw new Error(`Failed to extract ${partType} for timezone ${timeZone}`);
  }
  return value;
};

/** Format date as YYYY-MM-DD in a specific timezone */
export const formatDateInTimeZone = (date: Date, timeZone: string): string => {
  const year = getDatePart(date, timeZone, "year");
  const month = getDatePart(date, timeZone, "month");
  const day = getDatePart(date, timeZone, "day");
  return `${year}-${month}-${day}`;
};

export const addDays = (date: Date, days: number): Date => {
  const next = new Date(date);
  next.setUTCDate(next.getUTCDate() + days);
  return next;
};

/**
 * Convert a local date (YYYY-MM-DD) + timezone into a UTC ISO range.
 *
 * For example, "2026-02-28" in "Europe/Helsinki" (UTC+2 winter) becomes:
 *   startUtc = "2026-02-27T22:00:00.000Z"
 *   endUtc   = "2026-02-28T22:00:00.000Z"
 *
 * This is needed because Nord Pool stores delivery times in CET/CEST offsets,
 * and a LIKE 'YYYY-MM-DD%' query on the date prefix misses entries whose UTC
 * representation falls on the previous calendar day.
 */
export const getUtcRangeForLocalDate = (
  localDate: string,
  timeZone: string,
): { startUtc: string; endUtc: string } => {
  // Build a Date for midnight in the target timezone.
  // Intl.DateTimeFormat can tell us the UTC offset for that moment.
  const midnightLocal = new Date(`${localDate}T00:00:00`);

  // Use a formatter to find the UTC offset at midnight of this date
  const formatter = new Intl.DateTimeFormat("en-GB", {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
    timeZoneName: "longOffset",
  });

  const parts = formatter.formatToParts(midnightLocal);
  const tzName = parts.find((p) => p.type === "timeZoneName")?.value ?? "GMT";
  const offset = tzName === "GMT" ? "+00:00" : tzName.replace("GMT", "");

  // Parse offset like "+02:00" or "-05:00" into minutes
  const sign = offset.startsWith("-") ? -1 : 1;
  const [hStr, mStr] = offset.slice(1).split(":");
  const offsetMinutes = sign * (Number(hStr) * 60 + Number(mStr ?? "0"));

  // Midnight local = midnight UTC minus the offset
  const startMs =
    Date.UTC(
      Number(localDate.slice(0, 4)),
      Number(localDate.slice(5, 7)) - 1,
      Number(localDate.slice(8, 10)),
    ) -
    offsetMinutes * 60_000;

  const endMs = startMs + 24 * 60 * 60_000;

  return {
    startUtc: new Date(startMs).toISOString(),
    endUtc: new Date(endMs).toISOString(),
  };
};

/**
 * Format an ISO datetime string as a valid ISO 8601 timestamp with timezone offset.
 * Example: "2026-02-24T14:00:00+02:00"
 */
export const formatDateTimeInTimeZone = (
  isoDateTime: string,
  timeZone: string,
): string => {
  const date = new Date(isoDateTime);
  const formatter = new Intl.DateTimeFormat("en-GB", {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
    timeZoneName: "longOffset",
  });

  const parts = formatter.formatToParts(date);
  const get = (type: Intl.DateTimeFormatPartTypes): string =>
    parts.find((p) => p.type === type)?.value ?? "";

  const year = get("year");
  const month = get("month");
  const day = get("day");
  const hour = get("hour") === "24" ? "00" : get("hour");
  const minute = get("minute");
  const second = get("second");
  const tzName = get("timeZoneName"); // "GMT+02:00" or "GMT"

  // "GMT+02:00" → "+02:00", "GMT" (= UTC) → "+00:00"
  const offset = tzName === "GMT" ? "+00:00" : tzName.replace("GMT", "");

  return `${year}-${month}-${day}T${hour}:${minute}:${second}${offset}`;
};
