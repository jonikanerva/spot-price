import { afterEach, describe, expect, it } from "vitest";
import type Database from "better-sqlite3";
import { createAuth } from "./auth.js";
import { closeDatabase, initTestDatabase } from "./db.js";

const ORIGINAL_ENV = {
  NODE_ENV: process.env["NODE_ENV"],
  BETTER_AUTH_URL: process.env["BETTER_AUTH_URL"],
  BETTER_AUTH_SECRET: process.env["BETTER_AUTH_SECRET"],
};

const restoreEnv = (): void => {
  if (ORIGINAL_ENV.NODE_ENV === undefined) {
    delete process.env.NODE_ENV;
  } else {
    process.env.NODE_ENV = ORIGINAL_ENV.NODE_ENV;
  }

  if (ORIGINAL_ENV.BETTER_AUTH_URL === undefined) {
    delete process.env.BETTER_AUTH_URL;
  } else {
    process.env.BETTER_AUTH_URL = ORIGINAL_ENV.BETTER_AUTH_URL;
  }

  if (ORIGINAL_ENV.BETTER_AUTH_SECRET === undefined) {
    delete process.env.BETTER_AUTH_SECRET;
  } else {
    process.env.BETTER_AUTH_SECRET = ORIGINAL_ENV.BETTER_AUTH_SECRET;
  }
};

describe("createAuth", () => {
  let db: Database.Database;

  afterEach(() => {
    restoreEnv();
    closeDatabase(db);
  });

  it("throws in production when BETTER_AUTH_SECRET is missing", () => {
    db = initTestDatabase();
    process.env["NODE_ENV"] = "production";
    process.env["BETTER_AUTH_URL"] = "https://spot.example.com";
    delete process.env["BETTER_AUTH_SECRET"];

    expect(() => createAuth(db)).toThrow(
      "BETTER_AUTH_SECRET is required in production",
    );
  });

  it("throws in production when BETTER_AUTH_URL is missing", () => {
    db = initTestDatabase();
    process.env["NODE_ENV"] = "production";
    process.env["BETTER_AUTH_SECRET"] = "a-very-long-production-secret-value";
    delete process.env["BETTER_AUTH_URL"];

    expect(() => createAuth(db)).toThrow(
      "BETTER_AUTH_URL is required in production",
    );
  });

  it("allows dev mode defaults", () => {
    db = initTestDatabase();
    process.env["NODE_ENV"] = "development";
    delete process.env["BETTER_AUTH_URL"];
    delete process.env["BETTER_AUTH_SECRET"];

    expect(() => createAuth(db)).not.toThrow();
  });
});
