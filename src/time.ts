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
