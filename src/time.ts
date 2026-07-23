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
 * This is needed because Nord Pool delivery times may start before midnight UTC,
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
 * Advance a YYYY-MM-DD label by one calendar day.
 *
 * Pure UTC calendar arithmetic on the label parts (same `Date.UTC` pattern as
 * `getUtcRangeForLocalDate`): UTC has no DST, so adding 24h to a UTC-midnight
 * instant always lands on the next calendar day regardless of any wall-clock
 * transition in the target timezone. DST-immune by construction.
 */
const nextCalendarDay = (date: string): string =>
  formatUtcDate(
    new Date(
      Date.UTC(
        Number(date.slice(0, 4)),
        Number(date.slice(5, 7)) - 1,
        Number(date.slice(8, 10)),
      ) +
        24 * 60 * 60_000,
    ),
  );

/**
 * Convert an inclusive local date span (fromDate..toDate, YYYY-MM-DD) + timezone
 * into a single UTC ISO range covering every delivery interval in those local days.
 *
 * The start is the UTC instant of `fromDate` midnight local; the end is the UTC
 * instant of the *next* local midnight after `toDate` — i.e. the start of the day
 * following `toDate`, resolved with that day's own UTC offset. Using the next
 * day's start (rather than `toDate`'s hard +24h `endUtc`) keeps the span correct
 * across a DST fall-back `toDate`, where the local day is 25h long: the old
 * `endUtc` truncated the 25th hour. Each endpoint resolves its own UTC offset
 * independently via `getUtcRangeForLocalDate`, so a span crossing any DST
 * transition stays correct by construction (no shared offset across the span).
 */
export const getUtcRangeForLocalDateSpan = (
  fromDate: string,
  toDate: string,
  timeZone: string,
): { startUtc: string; endUtc: string } => ({
  startUtc: getUtcRangeForLocalDate(fromDate, timeZone).startUtc,
  endUtc: getUtcRangeForLocalDate(nextCalendarDay(toDate), timeZone).startUtc,
});

// `Intl.DateTimeFormat` construction is comparatively expensive, but a given
// instance is stateless across the dates passed to `formatToParts`. The
// price-history endpoint formats up to ~2976 intervals (two calls each) per
// request; building a fresh formatter per call breaches the STACK.md §4 100 ms
// p99 budget (measured ~335 ms). Memoising per timezone keeps the formatter
// construction O(timezones) instead of O(intervals).
const offsetFormatterCache = new Map<string, Intl.DateTimeFormat>();

const getOffsetFormatter = (timeZone: string): Intl.DateTimeFormat => {
  const cached = offsetFormatterCache.get(timeZone);
  if (cached) {
    return cached;
  }
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
  offsetFormatterCache.set(timeZone, formatter);
  return formatter;
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
  const formatter = getOffsetFormatter(timeZone);

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
