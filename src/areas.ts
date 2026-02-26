/** Nord Pool delivery area with timezone mapping */
export interface DeliveryArea {
  readonly code: string;
  readonly name: string;
  readonly country: string;
  readonly timezone: string;
}

/** All 21 Nord Pool delivery areas verified live on 2026-02-26 */
export const DELIVERY_AREAS: readonly DeliveryArea[] = [
  { code: "FI", name: "Finland", country: "FI", timezone: "Europe/Helsinki" },
  {
    code: "SE1",
    name: "Sweden — Luleå",
    country: "SE",
    timezone: "Europe/Stockholm",
  },
  {
    code: "SE2",
    name: "Sweden — Sundsvall",
    country: "SE",
    timezone: "Europe/Stockholm",
  },
  {
    code: "SE3",
    name: "Sweden — Stockholm",
    country: "SE",
    timezone: "Europe/Stockholm",
  },
  {
    code: "SE4",
    name: "Sweden — Malmö",
    country: "SE",
    timezone: "Europe/Stockholm",
  },
  {
    code: "NO1",
    name: "Norway — Oslo",
    country: "NO",
    timezone: "Europe/Oslo",
  },
  {
    code: "NO2",
    name: "Norway — Kristiansand",
    country: "NO",
    timezone: "Europe/Oslo",
  },
  {
    code: "NO3",
    name: "Norway — Trondheim",
    country: "NO",
    timezone: "Europe/Oslo",
  },
  {
    code: "NO4",
    name: "Norway — Tromsø",
    country: "NO",
    timezone: "Europe/Oslo",
  },
  {
    code: "NO5",
    name: "Norway — Bergen",
    country: "NO",
    timezone: "Europe/Oslo",
  },
  {
    code: "DK1",
    name: "Denmark — West",
    country: "DK",
    timezone: "Europe/Copenhagen",
  },
  {
    code: "DK2",
    name: "Denmark — East",
    country: "DK",
    timezone: "Europe/Copenhagen",
  },
  { code: "EE", name: "Estonia", country: "EE", timezone: "Europe/Tallinn" },
  {
    code: "LT",
    name: "Lithuania",
    country: "LT",
    timezone: "Europe/Vilnius",
  },
  { code: "LV", name: "Latvia", country: "LV", timezone: "Europe/Riga" },
  { code: "AT", name: "Austria", country: "AT", timezone: "Europe/Vienna" },
  { code: "BE", name: "Belgium", country: "BE", timezone: "Europe/Brussels" },
  { code: "FR", name: "France", country: "FR", timezone: "Europe/Paris" },
  { code: "GER", name: "Germany", country: "DE", timezone: "Europe/Berlin" },
  {
    code: "NL",
    name: "Netherlands",
    country: "NL",
    timezone: "Europe/Amsterdam",
  },
  { code: "PL", name: "Poland", country: "PL", timezone: "Europe/Warsaw" },
] as const;

/** Set of valid area codes for O(1) lookup */
export const VALID_AREA_CODES: ReadonlySet<string> = new Set(
  DELIVERY_AREAS.map((a) => a.code),
);

/** All valid area codes as a comma-separated string (for API calls) */
export const ALL_AREA_CODES_CSV: string = DELIVERY_AREAS.map(
  (a) => a.code,
).join(",");

/** Unique supported timezones derived from delivery areas */
export const SUPPORTED_TIMEZONES: readonly string[] = [
  ...new Set(DELIVERY_AREAS.map((a) => a.timezone)),
].sort();

/** Set of valid timezones for O(1) lookup */
const VALID_TIMEZONES: ReadonlySet<string> = new Set(SUPPORTED_TIMEZONES);

/** Check if a string is a valid Nord Pool delivery area code */
export const isValidAreaCode = (code: string): boolean =>
  VALID_AREA_CODES.has(code);

/** Check if a string is a valid supported timezone */
export const isValidTimezone = (tz: string): boolean => VALID_TIMEZONES.has(tz);

/** Get the default timezone for a delivery area code */
export const getDefaultTimezone = (areaCode: string): string => {
  const area = DELIVERY_AREAS.find((a) => a.code === areaCode);
  if (!area) {
    throw new Error(`Unknown area code: ${areaCode}`);
  }
  return area.timezone;
};

/** Get a delivery area by code, or undefined if not found */
export const getArea = (code: string): DeliveryArea | undefined =>
  DELIVERY_AREAS.find((a) => a.code === code);
