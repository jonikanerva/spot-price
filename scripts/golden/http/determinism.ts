// Pins the clock and seeds every random source of the TypeScript service, so
// a golden run writes the same bytes each time. Call `installDeterminism`
// before the first import of `src/` or `better-auth`: a module that runs
// before the patch can keep the real `Date` or the real random source.
import nodeCrypto from "node:crypto";
import { syncBuiltinESMExports } from "node:module";

let nowMs = 0;
let state = 0;

/** mulberry32: one byte per call from the seeded state. */
const nextByte = (): number => {
  state = (state + 0x6d2b79f5) >>> 0;
  let t = state;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) & 0xff;
};

const fill = (view: ArrayBufferView): void => {
  const bytes = new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = nextByte();
  }
};

/** Seeds the generator from a text label, so each case has its own stream. */
export const reseed = (label: string): void => {
  let h = 2166136261;
  for (const ch of label) {
    h = Math.imul(h ^ (ch.codePointAt(0) ?? 0), 16777619) >>> 0;
  }
  state = h;
};

export const setNow = (iso: string): void => {
  const ms = Date.parse(iso);
  if (!Number.isFinite(ms)) {
    throw new Error(`bad clock: ${iso}`);
  }
  nowMs = ms;
};

export const installDeterminism = (): void => {
  const RealDate = Date;
  class FixedDate extends RealDate {
    constructor(...args: unknown[]) {
      if (args.length === 0) {
        super(nowMs);
      } else {
        // reason: forwards the original Date constructor arguments unchanged.
        super(...(args as [number]));
      }
    }
    static override now(): number {
      return nowMs;
    }
  }
  globalThis.Date = FixedDate as DateConstructor;

  const randomBytes = (size: number, cb?: (e: null, b: Buffer) => void) => {
    const buf = Buffer.alloc(size);
    fill(buf);
    if (cb) {
      cb(null, buf);
      return undefined;
    }
    return buf;
  };
  // reason: replaces the builtin export; syncBuiltinESMExports() then updates
  // the ESM named bindings that `import { randomBytes }` already holds.
  (nodeCrypto as { randomBytes: unknown }).randomBytes = randomBytes;
  (nodeCrypto as { randomFillSync: unknown }).randomFillSync = <
    T extends ArrayBufferView,
  >(
    view: T,
  ): T => {
    fill(view);
    return view;
  };
  syncBuiltinESMExports();

  const getRandomValues = <T extends ArrayBufferView | null>(view: T): T => {
    if (view) {
      fill(view);
    }
    return view;
  };
  Object.defineProperty(globalThis.crypto, "getRandomValues", {
    value: getRandomValues,
    configurable: true,
    writable: true,
  });
  Object.defineProperty(nodeCrypto.webcrypto, "getRandomValues", {
    value: getRandomValues,
    configurable: true,
    writable: true,
  });
};
