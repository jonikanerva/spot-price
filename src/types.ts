/** Hourly price entry from Nord Pool Data Portal API */
export interface NordPoolEntry {
  readonly deliveryStart: string;
  readonly deliveryEnd: string;
  readonly entryPerArea: Readonly<Record<string, number>>;
}

/** Nord Pool Data Portal API response */
export interface NordPoolResponse {
  readonly deliveryDateCET: string;
  readonly updatedAt: string;
  readonly currency: string;
  readonly multiAreaEntries: readonly NordPoolEntry[];
  readonly areaStates: readonly {
    readonly state: string;
    readonly areas: readonly string[];
  }[];
}

/** Parsed hourly price in our domain */
export interface HourlyPrice {
  readonly deliveryStart: string;
  readonly deliveryEnd: string;
  readonly priceEurMwh: number;
  readonly area: string;
}

/** User electricity contract settings */
export interface UserSettings {
  readonly userId: string;
  readonly marginCentsKwh: number;
  readonly transferDayCentsKwh: number;
  readonly transferNightCentsKwh: number;
  readonly taxCentsKwh: number;
  readonly vatPercent: number;
  readonly nightStartHour: number;
  readonly nightEndHour: number;
  readonly timezone: string;
}

/** Total price breakdown for a single hour */
export interface TotalPrice {
  readonly deliveryStart: string;
  readonly deliveryEnd: string;
  readonly spotCentsKwh: number;
  readonly marginCentsKwh: number;
  readonly transferCentsKwh: number;
  readonly taxCentsKwh: number;
  readonly vatCentsKwh: number;
  readonly totalCentsKwh: number;
  readonly isNightRate: boolean;
  readonly hour: number;
}

/** Cheapest contiguous window result */
export interface CheapestWindow {
  readonly start: string;
  readonly end: string;
  readonly averageTotalCentsKwh: number;
  readonly prices: readonly TotalPrice[];
}
