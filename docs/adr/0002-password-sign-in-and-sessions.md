# 0002. Password sign-in keeps the Better Auth format; sessions are new

- Status: accepted
- Date: 2026-10-14

## Context

The setup UI signs in with a username and a password (`VISION.md`). The
TypeScript service used Better Auth. Better Auth stores a scrypt hash in
`account.password` and its sessions in the `"session"` table, with a signed
cookie. Home automations do not use sessions. They use API keys.

The default Rust profile (STACK-RUST §2.5) expects OIDC and opaque backend
sessions. No supported Rust library reads Better Auth sessions.

## Decision

1. Accounts stay password accounts. This is a recorded divergence from the
   OIDC default, because the product requires local accounts.
2. The Rust service verifies and creates password hashes in the Better Auth
   format: `<salt>:<key>`, where the salt is 16 random bytes as hex text and
   the hex text itself is the scrypt salt, and the key is
   scrypt(N=16384, r=16, p=1, 64 bytes) of the NFKC-normalised password, as
   hex. Existing passwords keep working.
3. Sign-up writes the `"user"`, `"account"`, `usernames`, and
   `user_settings` rows in the format that Better Auth wrote. The TypeScript
   service can sign these users in after a revert.
4. Sessions move to a new table, `app_sessions`. The table stores only a
   SHA-256 hash of an opaque random token, the user id, and two instants. It
   stores no IP address and no user agent. Each user signs in once after the
   release. The old `"session"` table stays untouched.
5. Session routes that change state reject a request with a foreign
   `Origin` header. The cookie is `HttpOnly`, `SameSite=Lax`, and `Secure`
   with the `__Host-` prefix on HTTPS.
6. API keys do not change. Automations need no change.

## Alternatives

- **Read Better Auth session cookies.** Rejected by the owner: more code
  and risk to avoid one sign-in per user.
- **Reset all passwords.** Rejected: users would lose access.
- **OIDC.** Rejected: a new external provider for a single-household
  service, against the product shape.

## Consequences

- scrypt needs about 32 MiB of memory per hash. The service runs it on the
  blocking pool behind a semaphore of two.
- The session code is first-party. It needs the independent security review
  of this PR and the tests of fixation, expiry, and sign-out.

## Cost of reversal

Low. The password format is the Better Auth format, so either service can
own the accounts.

## Reassessment

When the product needs another sign-in method, or when the revert path to
the TypeScript service ends (ADR 0001).
