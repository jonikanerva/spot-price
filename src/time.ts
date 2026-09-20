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

/**
 * Shift a UTC instant by whole UTC days, preserving the UTC time of day.
 *
 * This is UTC arithmetic on an instant. It is not calendar arithmetic on a
 * local date label: a local calendar day is 23h or 25h long around a DST
 * transition, so formatting the result of `addDays` in a local timezone can
 * land on the wrong date. Use `getCurrentAndNextDate` for local date labels.
 */
export const addDays = (date: Date, days: number): Date => {
  const next = new Date(date);
  next.setUTCDate(next.getUTCDate() + days);
  return next;
};

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

// --- Local calendar days ---------------------------------------------------
//
// One concept, one owner. `startOfLocalDayUtc` answers "at which UTC instant
// does this local calendar day begin?", and every helper below is built from it
// plus `nextCalendarDay`. No helper assumes a local day is 24 hours long: it is
// 23 hours on a spring-forward date and 25 hours on a fall-back date.

/**
 * Advance a YYYY-MM-DD label by one calendar day.
 *
 * Pure UTC calendar arithmetic on the label parts: UTC has no DST, so adding
 * 24h to a UTC-midnight instant always lands on the next calendar day,
 * regardless of any wall-clock transition in the target timezone. DST-immune by
 * construction.
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
 * The UTC instant at which a local calendar day starts.
 *
 * Reads the timezone's UTC offset at that day's midnight and subtracts it from
 * UTC midnight of the same label.
 *
 * Two properties of the probe instant are load-bearing:
 *
 * 1. It carries a `Z`. Without it the string parses in the HOST timezone, which
 *    resolves the wrong offset on a transition date — one hour off under
 *    `TZ=America/New_York`, correct under `TZ=UTC`.
 * 2. It stays at midnight. EU timezones transition at 01:00 UTC, so a midday
 *    probe reads the post-transition offset and starts a fall-back day one hour
 *    late. `SUPPORTED_TIMEZONES` derives from `DELIVERY_AREAS` (`src/areas.ts`)
 *    and every zone in it transitions at 01:00 UTC, so the probe instant and
 *    the local midnight it stands for always fall on the same side of the
 *    transition. A non-EU delivery area would break that guarantee.
 */
const startOfLocalDayUtc = (localDate: string, timeZone: string): Date => {
  const midnightProbe = new Date(`${localDate}T00:00:00Z`);

  const parts = getOffsetFormatter(timeZone).formatToParts(midnightProbe);
  const tzName = parts.find((p) => p.type === "timeZoneName")?.value ?? "GMT";
  const offset = tzName === "GMT" ? "+00:00" : tzName.replace("GMT", "");

  // Parse offset like "+02:00" or "-05:00" into minutes
  const sign = offset.startsWith("-") ? -1 : 1;
  const [hStr, mStr] = offset.slice(1).split(":");
  const offsetMinutes = sign * (Number(hStr) * 60 + Number(mStr ?? "0"));

  // Midnight local = midnight UTC minus the offset
  return new Date(
    Date.UTC(
      Number(localDate.slice(0, 4)),
      Number(localDate.slice(5, 7)) - 1,
      Number(localDate.slice(8, 10)),
    ) -
      offsetMinutes * 60_000,
  );
};

/**
 * Get today and tomorrow date strings in the given timezone.
 *
 * `tomorrow` is calendar arithmetic on today's LABEL, not a 24h shift of the
 * instant. A 24h shift is wrong twice a year: on a fall-back day it stays
 * inside the same 25-hour local date, so `tomorrow` equals `today`; on a
 * spring-forward day it jumps over the 23-hour local date, so that date
 * disappears.
 */
export const getCurrentAndNextDate = (
  timeZone: string,
): { today: string; tomorrow: string } => {
  const today = formatDateInTimeZone(new Date(), timeZone);
  return { today, tomorrow: nextCalendarDay(today) };
};

/**
 * Convert a local date (YYYY-MM-DD) + timezone into a UTC ISO range covering
 * exactly that local calendar day.
 *
 * For example, "2026-02-28" in "Europe/Helsinki" (UTC+2 winter) becomes:
 *   startUtc = "2026-02-27T22:00:00.000Z"
 *   endUtc   = "2026-02-28T22:00:00.000Z"
 *
 * The end is the start of the *next* local day, resolved with that day's own
 * UTC offset, so the range is as long as the local day really is. A fixed +24h
 * end truncates the 25th hour of a fall-back day and overshoots a
 * spring-forward day by one hour.
 *
 * The UTC range is needed because Nord Pool delivery times may start before
 * midnight UTC, and a LIKE 'YYYY-MM-DD%' query on the date prefix misses
 * entries whose UTC representation falls on the previous calendar day.
 */
export const getUtcRangeForLocalDate = (
  localDate: string,
  timeZone: string,
): { startUtc: string; endUtc: string } => ({
  startUtc: startOfLocalDayUtc(localDate, timeZone).toISOString(),
  endUtc: startOfLocalDayUtc(
    nextCalendarDay(localDate),
    timeZone,
  ).toISOString(),
});

/**
 * Convert an inclusive local date span (fromDate..toDate, YYYY-MM-DD) + timezone
 * into a single UTC ISO range covering every delivery interval in those local days.
 *
 * The start is the UTC instant of `fromDate` midnight local; the end is the UTC
 * instant of the next local midnight after `toDate`. Each endpoint resolves its
 * own UTC offset independently, so a span crossing any DST transition stays
 * correct by construction (no shared offset across the span).
 */
export const getUtcRangeForLocalDateSpan = (
  fromDate: string,
  toDate: string,
  timeZone: string,
): { startUtc: string; endUtc: string } => ({
  startUtc: startOfLocalDayUtc(fromDate, timeZone).toISOString(),
  endUtc: startOfLocalDayUtc(nextCalendarDay(toDate), timeZone).toISOString(),
});

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
