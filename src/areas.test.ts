import { describe, expect, it } from "vitest";
import {
  DELIVERY_AREAS,
  SUPPORTED_TIMEZONES,
  VALID_AREA_CODES,
  getArea,
  getDefaultTimezone,
  isValidAreaCode,
  isValidTimezone,
} from "./areas.js";

describe("DELIVERY_AREAS", () => {
  it("contains exactly 21 areas", () => {
    expect(DELIVERY_AREAS).toHaveLength(21);
  });

  it("has unique area codes", () => {
    const codes = DELIVERY_AREAS.map((a) => a.code);
    expect(new Set(codes).size).toBe(codes.length);
  });

  it("includes FI as first entry", () => {
    expect(DELIVERY_AREAS.at(0)).toEqual(
      expect.objectContaining({ code: "FI", timezone: "Europe/Helsinki" }),
    );
  });

  it("every area has non-empty code, name, country, and timezone", () => {
    for (const area of DELIVERY_AREAS) {
      expect(area.code.length).toBeGreaterThan(0);
      expect(area.name.length).toBeGreaterThan(0);
      expect(area.country.length).toBeGreaterThan(0);
      expect(area.timezone.length).toBeGreaterThan(0);
    }
  });
});

describe("SUPPORTED_TIMEZONES", () => {
  it("contains exactly 13 unique timezones", () => {
    expect(SUPPORTED_TIMEZONES).toHaveLength(13);
  });

  it("is sorted alphabetically", () => {
    const sorted = [...SUPPORTED_TIMEZONES].sort();
    expect(SUPPORTED_TIMEZONES).toEqual(sorted);
  });

  it("includes Europe/Helsinki", () => {
    expect(SUPPORTED_TIMEZONES).toContain("Europe/Helsinki");
  });

  it("includes Europe/Stockholm", () => {
    expect(SUPPORTED_TIMEZONES).toContain("Europe/Stockholm");
  });

  it("does not include non-European timezones", () => {
    for (const tz of SUPPORTED_TIMEZONES) {
      expect(tz).toMatch(/^Europe\//);
    }
  });
});

describe("isValidAreaCode", () => {
  it("returns true for valid area codes", () => {
    expect(isValidAreaCode("FI")).toBe(true);
    expect(isValidAreaCode("SE1")).toBe(true);
    expect(isValidAreaCode("SE4")).toBe(true);
    expect(isValidAreaCode("NO1")).toBe(true);
    expect(isValidAreaCode("NO5")).toBe(true);
    expect(isValidAreaCode("DK1")).toBe(true);
    expect(isValidAreaCode("EE")).toBe(true);
    expect(isValidAreaCode("GER")).toBe(true);
    expect(isValidAreaCode("PL")).toBe(true);
  });

  it("returns false for invalid area codes", () => {
    expect(isValidAreaCode("")).toBe(false);
    expect(isValidAreaCode("INVALID")).toBe(false);
    expect(isValidAreaCode("fi")).toBe(false);
    expect(isValidAreaCode("SYS")).toBe(false);
    expect(isValidAreaCode("TEL")).toBe(false);
    expect(isValidAreaCode("US")).toBe(false);
  });
});

describe("isValidTimezone", () => {
  it("returns true for supported timezones", () => {
    expect(isValidTimezone("Europe/Helsinki")).toBe(true);
    expect(isValidTimezone("Europe/Stockholm")).toBe(true);
    expect(isValidTimezone("Europe/Oslo")).toBe(true);
    expect(isValidTimezone("Europe/Berlin")).toBe(true);
    expect(isValidTimezone("Europe/Warsaw")).toBe(true);
  });

  it("returns false for unsupported timezones", () => {
    expect(isValidTimezone("")).toBe(false);
    expect(isValidTimezone("US/Pacific")).toBe(false);
    expect(isValidTimezone("America/New_York")).toBe(false);
    expect(isValidTimezone("UTC")).toBe(false);
    expect(isValidTimezone("Europe/London")).toBe(false);
  });
});

describe("getDefaultTimezone", () => {
  it("returns Europe/Helsinki for FI", () => {
    expect(getDefaultTimezone("FI")).toBe("Europe/Helsinki");
  });

  it("returns Europe/Stockholm for all Swedish areas", () => {
    expect(getDefaultTimezone("SE1")).toBe("Europe/Stockholm");
    expect(getDefaultTimezone("SE2")).toBe("Europe/Stockholm");
    expect(getDefaultTimezone("SE3")).toBe("Europe/Stockholm");
    expect(getDefaultTimezone("SE4")).toBe("Europe/Stockholm");
  });

  it("returns Europe/Oslo for all Norwegian areas", () => {
    expect(getDefaultTimezone("NO1")).toBe("Europe/Oslo");
    expect(getDefaultTimezone("NO2")).toBe("Europe/Oslo");
    expect(getDefaultTimezone("NO3")).toBe("Europe/Oslo");
    expect(getDefaultTimezone("NO4")).toBe("Europe/Oslo");
    expect(getDefaultTimezone("NO5")).toBe("Europe/Oslo");
  });

  it("returns Europe/Copenhagen for Danish areas", () => {
    expect(getDefaultTimezone("DK1")).toBe("Europe/Copenhagen");
    expect(getDefaultTimezone("DK2")).toBe("Europe/Copenhagen");
  });

  it("returns correct timezone for Baltic states", () => {
    expect(getDefaultTimezone("EE")).toBe("Europe/Tallinn");
    expect(getDefaultTimezone("LT")).toBe("Europe/Vilnius");
    expect(getDefaultTimezone("LV")).toBe("Europe/Riga");
  });

  it("returns correct timezone for Central European areas", () => {
    expect(getDefaultTimezone("AT")).toBe("Europe/Vienna");
    expect(getDefaultTimezone("BE")).toBe("Europe/Brussels");
    expect(getDefaultTimezone("FR")).toBe("Europe/Paris");
    expect(getDefaultTimezone("GER")).toBe("Europe/Berlin");
    expect(getDefaultTimezone("NL")).toBe("Europe/Amsterdam");
    expect(getDefaultTimezone("PL")).toBe("Europe/Warsaw");
  });

  it("throws for unknown area code", () => {
    expect(() => getDefaultTimezone("INVALID")).toThrow("Unknown area code");
    expect(() => getDefaultTimezone("")).toThrow("Unknown area code");
  });
});

describe("getArea", () => {
  it("returns area object for valid code", () => {
    const fi = getArea("FI");
    expect(fi).toBeDefined();
    expect(fi?.code).toBe("FI");
    expect(fi?.name).toBe("Finland");
    expect(fi?.country).toBe("FI");
    expect(fi?.timezone).toBe("Europe/Helsinki");
  });

  it("returns undefined for invalid code", () => {
    expect(getArea("INVALID")).toBeUndefined();
    expect(getArea("")).toBeUndefined();
  });
});

describe("VALID_AREA_CODES", () => {
  it("contains exactly 21 entries", () => {
    expect(VALID_AREA_CODES.size).toBe(21);
  });

  it("is consistent with DELIVERY_AREAS", () => {
    for (const area of DELIVERY_AREAS) {
      expect(VALID_AREA_CODES.has(area.code)).toBe(true);
    }
  });
});
