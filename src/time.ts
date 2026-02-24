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
