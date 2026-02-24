# Implementation Plan: UI/UX Username-First Phase

Date: 2026-02-24
Owner: Repository Owner + Agent
Status: Approved for execution

## Goal

Deliver a developer-focused dark-mode UI where users authenticate with username + password using a single "Login or Signup" action, see public spot-price charts on landing, and manage personalized pricing + API usage after login.

## Scope

### In scope

- Username-only auth UX (no email input in UI)
- Single action login/signup flow
- Public landing line chart (today + tomorrow when available, 15-min, c/kWh)
- Authenticated dashboard layout: total-price chart (left) + settings panel (right)
- API panel with key management and REST usage examples
- Dark-mode-only visual system

### Out of scope

- Account recovery workflow
- Multi-theme support
- Mobile app

## Milestones

1. Username auth backend compatibility layer (Better Auth under the hood)
2. Public spot chart endpoint for landing
3. Session-protected dashboard chart endpoint (total price by user settings)
4. Dark-mode landing redesign (login-only + public chart)
5. Authenticated dashboard redesign (settings + total chart + API panel)
6. Test and production verification pass

## Acceptance criteria

- Landing page shows only `username`, `password`, and one `Login or Signup` button
- Landing chart shows 15-min spot prices in c/kWh for today and tomorrow (if available)
- Logged-in page shows settings panel on right and personalized total-price chart on left
- API panel exposes current key and ready-to-copy REST examples
- UI is dark-mode-only and remains usable on desktop + mobile
