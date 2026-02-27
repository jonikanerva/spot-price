# Research Dossier: Home Assistant Integration Patterns for Spot Price API

Date: 2026-02-27
Author: Research Agent
Status: **Draft** — awaiting owner review

---

## Problem Statement

Home Assistant users with spot-priced electricity contracts need to automate flexible loads (EV charging, water heaters, dishwashers) to run during the cheapest hours. The Spot Price API provides total-cost-aware price data and a server-side cheapest-window calculation, but users need a clear, documented path to connect this API to their HA automations.

The core use case: **"Charge my EV (or run a flexible load) during the cheapest N hours overnight."**

## Target Users

| User                                  | Job-to-be-done                                                            |
| ------------------------------------- | ------------------------------------------------------------------------- |
| HA enthusiast (YAML-comfortable)      | Configure REST sensors + automations to schedule loads during cheap hours |
| HA beginner (UI-only)                 | Follow a copy-paste guide to set up price-based automation                |
| Advanced HA user (Node-RED/AppDaemon) | Build custom flows using the API for complex scheduling logic             |
| EV owner with HA                      | Automatically charge EV during cheapest overnight window                  |

---

## Topic 1: HA Integration Methods for REST APIs

### Method A: RESTful Sensor (`rest:` / `sensor.rest`)

**How it works**: Built-in HA integration that polls a REST endpoint on a configurable interval. Returns JSON data that can be parsed with `value_template` and `json_attributes`. Multiple sensors can share a single HTTP request via the `rest:` platform.

**Conceptual YAML for our API:**

```yaml
rest:
  - resource: "https://spot.calmdonut.com/api/v1/price/now"
    scan_interval: 900 # 15 minutes
    headers:
      Authorization: "Bearer YOUR_API_KEY"
    sensor:
      - name: "Spot Price Total"
        value_template: "{{ value_json.totalCentsKwh }}"
        unit_of_measurement: "c/kWh"
        device_class: monetary
        json_attributes:
          - spotCentsKwh
          - marginCentsKwh
          - transferCentsKwh
          - taxCentsKwh
          - vatCentsKwh
          - isNightRate
          - deliveryStart
          - deliveryEnd
```

| Attribute             | Assessment                                                                                                                |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| **Polling frequency** | Configurable via `scan_interval` (default 30s). 900s (15 min) appropriate for 15-min resolution                           |
| **Auth support**      | Yes — `headers:` with Bearer token, or `username`/`password` for basic auth                                               |
| **JSON parsing**      | Yes — `value_template` with `value_json`, `json_attributes`, `json_attributes_path`                                       |
| **Complexity**        | Low — YAML only, well-documented, used by 7.5% of HA installations                                                        |
| **Cheapest window**   | Cannot compute cheapest window itself, but can poll our `/price/cheapest` endpoint                                        |
| **Survives restart**  | Yes — state restored from HA database                                                                                     |
| **Limitations**       | Polling only (no push). State value limited to 255 chars (use attributes for complex data). Cannot store arrays as state. |

**Evidence**: HA REST integration docs confirm `headers` support for Bearer tokens, `json_attributes` for extracting multiple fields, and `scan_interval` for polling control. Used by 7.5% of active installations. [Source: home-assistant.io/integrations/rest/, home-assistant.io/integrations/sensor.rest/]

### Method B: RESTful Command (`rest_command`)

**How it works**: Defines named HTTP actions that can be called from automations/scripts. Supports templates in URL, headers, and payload. Returns response data via `response_variable` for use in automation sequences.

**Conceptual YAML:**

```yaml
rest_command:
  get_cheapest_window:
    url: "https://spot.calmdonut.com/api/v1/price/cheapest?duration={{ duration }}&startTime={{ start }}&endTime={{ end }}"
    method: GET
    headers:
      Authorization: "Bearer YOUR_API_KEY"
    content_type: "application/json"
```

| Attribute             | Assessment                                                                                                        |
| --------------------- | ----------------------------------------------------------------------------------------------------------------- |
| **Polling frequency** | On-demand only (called from automations)                                                                          |
| **Auth support**      | Yes — headers with Bearer token                                                                                   |
| **Complexity**        | Low-medium — requires automation to call and process response                                                     |
| **Cheapest window**   | Yes — can call `/price/cheapest` with dynamic parameters                                                          |
| **Response handling** | `response_variable` gives access to status, content, and headers in automations                                   |
| **Limitations**       | Not a sensor — doesn't create entities. Must be combined with template sensors or input helpers to store results. |

**Evidence**: HA rest_command docs confirm `response_variable` support for accessing response data in automations. Used by 5.1% of active installations. [Source: home-assistant.io/integrations/rest_command/]

### Method C: Template Sensors (derived from REST data)

**How it works**: Creates derived sensors from other sensor data using Jinja2 templates. Can be state-based (auto-updates when source changes) or trigger-based (updates on schedule/event). Essential for creating binary sensors from price data.

**Conceptual YAML for "cheap now" binary sensor:**

```yaml
template:
  - sensor:
      - name: "Electricity Total Price"
        unit_of_measurement: "c/kWh"
        state: "{{ state_attr('sensor.spot_price_total', 'totalCentsKwh') | float(0) }}"
  - binary_sensor:
      - name: "Cheap Electricity Now"
        state: >
          {{ states('sensor.spot_price_total') | float(99) < 10.0 }}
        device_class: power
```

| Attribute              | Assessment                                                                        |
| ---------------------- | --------------------------------------------------------------------------------- |
| **Complexity**         | Low — Jinja2 templates, well-documented                                           |
| **Use for scheduling** | Essential — creates binary sensors that automations can trigger on                |
| **Trigger-based**      | Yes — can use `time_pattern`, `state`, or `event` triggers for controlled updates |
| **Limitations**        | Depends on source sensor data; complex Jinja2 for array processing                |

**Evidence**: HA template integration docs confirm trigger-based templates with `actions` for calling services and processing response data. [Source: home-assistant.io/integrations/template/]

### Method D: Trigger-Based Template with REST Command (Combined Pattern)

**How it works**: A trigger-based template sensor that periodically calls a `rest_command` (or uses the `action` block to call any service), processes the response, and stores the result as sensor state/attributes. This is the **most powerful built-in pattern** for our use case.

**Conceptual YAML:**

```yaml
template:
  - triggers:
      - trigger: time_pattern
        hours: "/1" # Every hour
      - trigger: time
        at: "14:15:00" # After tomorrow's prices are published
    actions:
      - action: rest_command.get_cheapest_window
        data:
          duration: "180"
          start: "{{ now().replace(hour=22, minute=0, second=0).isoformat() }}"
          end: "{{ (now() + timedelta(days=1)).replace(hour=7, minute=0, second=0).isoformat() }}"
        response_variable: cheapest
    sensor:
      - name: "Cheapest 3h Window Start"
        state: "{{ cheapest.content.startLocal }}"
        attributes:
          end: "{{ cheapest.content.endLocal }}"
          average_price: "{{ cheapest.content.averageTotalCentsKwh }}"
    binary_sensor:
      - name: "Cheapest Window Active"
        state: >
          {{ now().isoformat() >= cheapest.content.start
             and now().isoformat() < cheapest.content.end }}
```

| Attribute            | Assessment                                         |
| -------------------- | -------------------------------------------------- |
| **Complexity**       | Medium — combines trigger, action, and template    |
| **Cheapest window**  | Yes — full end-to-end scheduling                   |
| **Survives restart** | Yes — trigger-based sensor state is restored       |
| **Limitations**      | Requires understanding of Jinja2 datetime handling |

### Method E: Command Line Sensor

**How it works**: Executes a shell command (e.g., `curl`) and uses the output as sensor state. Can parse JSON output with `value_template` and `json_attributes`.

**Conceptual YAML:**

```yaml
command_line:
  - sensor:
      name: "Spot Price Now"
      command: 'curl -sS -H "Authorization: Bearer YOUR_KEY" https://spot.calmdonut.com/api/v1/price/now'
      value_template: "{{ value_json.totalCentsKwh }}"
      json_attributes:
        - spotCentsKwh
        - isNightRate
      scan_interval: 900
      unit_of_measurement: "c/kWh"
```

| Attribute        | Assessment                                                                                                          |
| ---------------- | ------------------------------------------------------------------------------------------------------------------- |
| **Complexity**   | Low — familiar to Linux users                                                                                       |
| **Auth support** | Yes — via curl headers                                                                                              |
| **Limitations**  | Requires `curl` in HA container. Less elegant than REST sensor. 15s command timeout default. Legacy quality rating. |
| **When to use**  | Fallback when REST sensor doesn't work for some reason                                                              |

**Evidence**: HA command_line docs confirm JSON attribute support and configurable scan_interval. Used by 4.2% of installations. Rated "Legacy" quality. [Source: home-assistant.io/integrations/command_line/]

### Method F: Node-RED

**How it works**: Flow-based automation tool with HTTP request nodes. Can call REST APIs, parse JSON, and trigger HA entities/services. Runs as an HA add-on.

| Attribute           | Assessment                                                                    |
| ------------------- | ----------------------------------------------------------------------------- |
| **Complexity**      | Medium — visual flow editor, but requires Node-RED add-on installation        |
| **Cheapest window** | Yes — HTTP request node can call any endpoint                                 |
| **Community**       | Large Node-RED + HA community                                                 |
| **Limitations**     | Separate system to maintain. Not native HA. Overkill for simple REST polling. |
| **When to use**     | Users already running Node-RED who prefer visual flows                        |

### Method G: AppDaemon / pyscript

**How it works**: Python-based automation engines that run alongside HA. Can make HTTP requests, process data with full Python, and control HA entities.

| Attribute       | Assessment                                                                         |
| --------------- | ---------------------------------------------------------------------------------- |
| **Complexity**  | High — requires Python knowledge, separate runtime                                 |
| **Power**       | Maximum — full Python for complex scheduling logic                                 |
| **Limitations** | Overkill for our use case. Separate add-on. Small community compared to native HA. |
| **When to use** | Advanced users with complex multi-device scheduling needs                          |

### UI Configurability Assessment

A critical question for usability: **can each method be set up entirely through the HA graphical interface, or does it require editing YAML files on the server?**

| Component                                      | UI Setup? | Detail                                                                                                                                |
| ---------------------------------------------- | :-------: | ------------------------------------------------------------------------------------------------------------------------------------- |
| **REST Sensor** (`rest:`)                      |  ❌ YAML  | No config flow. Must be added to `configuration.yaml`. Requires HA restart after changes.                                             |
| **REST Binary Sensor**                         |  ❌ YAML  | Same as REST sensor — YAML only.                                                                                                      |
| **REST Command**                               |  ❌ YAML  | Must be defined in `configuration.yaml`. Can be reloaded without restart. Once defined, callable from UI-created automations/scripts. |
| **Template Sensors**                           |  ❌ YAML  | `template:` platform is YAML only. No UI editor for template sensors.                                                                 |
| **Command Line Sensor**                        |  ❌ YAML  | YAML only.                                                                                                                            |
| **Automations**                                |   ✅ UI   | Full visual editor: triggers, conditions, actions. Can call `rest_command` actions from UI. Used by 99% of installations.             |
| **Scripts** (with parameters)                  |   ✅ UI   | Full visual editor with typed parameter fields. Can accept inputs and call services. Used by 99% of installations.                    |
| **Input Helpers** (datetime, number)           |   ✅ UI   | Created via Settings > Helpers. Used by 15.1% of installations. No YAML needed.                                                       |
| **Node-RED**                                   |   ✅ UI   | Visual flow editor (separate add-on UI).                                                                                              |
| **HACS Integrations** (e.g. EV Smart Charging) |   ✅ UI   | Config flow wizard in HA UI after HACS install.                                                                                       |

**Key insight**: The **data source layer** (sensors, REST commands) is YAML-only. But the **automation/action layer** (automations, scripts, input helpers) is fully UI-configurable. This means a practical setup requires **a small YAML bootstrap** (~10 lines to define the REST command) and then everything else can be done in the UI.

**Evidence**: HA REST integration docs show no config flow (YAML only). HA automation editor docs confirm full UI support for creating automations with triggers, conditions, and actions. HA input_datetime docs confirm UI creation via Settings > Helpers. HA script docs confirm UI editor with typed fields. [Source: home-assistant.io/integrations/rest/, home-assistant.io/docs/automation/editor/, home-assistant.io/integrations/input_datetime/, home-assistant.io/integrations/script/]

### Comparison Matrix: Integration Methods

| Criterion           |  REST Sensor   |  REST Command  | Template + Action |  Command Line  |  Node-RED   | AppDaemon |
| ------------------- | :------------: | :------------: | :---------------: | :------------: | :---------: | :-------: |
| Creates entities    |       ✅       |       ❌       |        ✅         |       ✅       |  Via nodes  |  Via API  |
| Polling support     |       ✅       | ❌ (on-demand) |   ✅ (trigger)    |       ✅       |     ✅      |    ✅     |
| Bearer auth         |       ✅       |       ✅       |    Via action     |   ✅ (curl)    |     ✅      |    ✅     |
| JSON parsing        |       ✅       |       ✅       |        ✅         |       ✅       |     ✅      |    ✅     |
| Cheapest window E2E | ❌ (poll only) |    Partial     |      **✅**       | ❌ (poll only) |     ✅      |    ✅     |
| **UI configurable** |   ❌ (YAML)    |   ❌ (YAML)    |     ❌ (YAML)     |   ❌ (YAML)    | ✅ (own UI) | ❌ (code) |
| Complexity          |    **Low**     |    Low-Med     |    **Medium**     |      Low       |   Medium    |   High    |
| HA installations    |      7.5%      |      5.1%      |     Built-in      |      4.2%      |   Add-on    |  Add-on   |
| Survives restart    |       ✅       |      N/A       |        ✅         |       ✅       |     ✅      |    ✅     |
| No extra add-ons    |       ✅       |       ✅       |        ✅         |       ✅       |     ❌      |    ❌     |

---

## Topic 2: Existing HA Integrations for Spot Price Electricity

### Nord Pool (Official, Platinum Tier)

**What it provides**: Current, previous, and next hour spot prices for any Nord Pool delivery area. Daily average, lowest, highest prices. Peak/off-peak block averages. Actions to get prices for any date. Used by 3,471 active installations.

**Key details**:

- Prices are **base energy price only** — no VAT, no transfer fees, no margin
- Hourly resolution (transitioning to 15-minute MTU)
- `nordpool.get_prices_for_date` action returns historical/tomorrow prices
- `nordpool.get_price_indices_for_date` action with configurable resolution
- Data polled hourly from the same `dataportal-api.nordpoolgroup.com` API our service uses

**How our API complements it**: Nord Pool gives raw spot prices. Our API adds the **total cost** (spot + margin + transfer + tax + VAT) based on user's contract settings, and provides the **cheapest window calculation** server-side. Users who already have Nord Pool installed could use our API specifically for the total-cost calculation and cheapest-window scheduling.

**Evidence**: HA Nord Pool docs confirm base-price-only output, hourly polling, and action-based price retrieval. Platinum quality scale. Introduced in HA 2024.12. [Source: home-assistant.io/integrations/nordpool/]

### Cheapest Energy Hours (HACS Template Macro)

**What it is**: A Jinja2 macro (not an integration) that finds the cheapest block of hours from price data. Installed via HACS as a custom template. 165 GitHub stars, 98 releases (v7.2.3 as of Feb 2026), actively maintained.

**How it works**: You call the macro in a template sensor, passing price data from any source (Nord Pool, ENTSO-E, etc.), and it returns the cheapest contiguous block of N hours. It's a **client-side calculation** done in Jinja2 within HA.

**Key insight**: This macro solves the **exact same problem** as our `/api/v1/price/cheapest` endpoint, but client-side in Jinja2. Our API does it server-side with the added benefit of total-cost awareness (including transfer fees, tax, VAT).

**Limitations**: Complex Jinja2 templates. Requires a price source sensor. No total-cost awareness — works with raw spot prices only unless the user manually adds costs.

**Evidence**: GitHub repo shows 165 stars, v7.2.3 (Feb 2026), GPL-3.0 license. HACS Default category. Works with Nord Pool, ENTSO-E, and other price sources. [Source: github.com/TheFes/cheapest-energy-hours]

### EV Smart Charging (HACS Integration)

**What it is**: A full HACS integration specifically for EV charging optimization. 279 GitHub stars, 41 forks, MIT license, actively maintained (v2.5.1, Oct 2025).

**How it works**: Takes a price sensor as input, calculates optimal 15-minute charging intervals, and outputs a binary `sensor.ev_smart_charging_charging` (on/off) that controls the charger. Supports continuous or non-continuous charging sessions, price limits, SOC-aware scheduling, and completion time targets.

**Supported price sources**: Native support for HACS Nord Pool, Energi Data Service, GE-Spot, ENTSO-E, and TGE integrations. Also supports a **generic price format** via template sensors — any source providing `[{start, end, value}]` data.

**Key features relevant to us**:

- 15-minute interval optimization
- Configurable charge start time and completion time
- Continuous vs. non-continuous charging preference
- Price limit support
- SOC-aware (knows when to stop)
- `charging_schedule` attribute for visualization

**How our API could integrate**: Our `/api/v1/price/today` and `/api/v1/price/tomorrow` endpoints return price data that could be formatted into the generic price format via a template sensor. However, EV Smart Charging already does its own cheapest-window calculation internally — it doesn't need our `/price/cheapest` endpoint.

**Evidence**: GitHub repo shows 279 stars, 224 commits, MIT license. Supports generic price format for any price source. [Source: github.com/jonasbkarlsson/ev_smart_charging]

### ENTSO-E Transparency Platform (HACS Integration)

**What it is**: HACS integration fetching day-ahead prices from ENTSO-E for all European countries. 259 GitHub stars, 60 forks, Apache-2.0 license.

**How it works**: Requires ENTSO-E API key (free registration). Provides current price, daily min/max/average, next hour price, and 24-hour forecast in attributes. Includes a **Price Modifier Template** for adding VAT, fixed costs, and currency conversion.

**Relevance**: Competes with our API for the same data (European spot prices). However, it requires users to configure their own cost modifier template — our API handles this server-side.

**Evidence**: GitHub repo shows 259 stars, v0.7.5 (Feb 2026). [Source: github.com/JaccoR/hass-entso-e]

### Tibber (Official Integration)

**What it is**: Official HA integration for Tibber energy customers. Provides current price, real-time consumption (with Pulse/Watty), and a `tibber.get_prices` action for hourly prices.

**How it handles scheduling**: Tibber itself doesn't schedule loads in HA. It provides price data and a `max_price`/`min_price` attribute. Users create their own automations using price thresholds. The Tibber app has its own smart charging feature for supported chargers, but this is outside HA.

**Evidence**: HA Tibber docs confirm `get_prices` action with response data, `max_price`/`min_price` attributes. Used by 2% of installations. [Source: home-assistant.io/integrations/tibber/]

### Comparison: Existing Integrations

| Integration               |       Type       | Price Source  |      Total Cost       |   Cheapest Window    | EV Scheduling | Stars/Installs |
| ------------------------- | :--------------: | :-----------: | :-------------------: | :------------------: | :-----------: | :------------: |
| **Nord Pool** (official)  |       Core       | Nord Pool API |    ❌ (spot only)     |          ❌          |      ❌       | 3,471 installs |
| **Cheapest Energy Hours** |    HACS macro    |  Any sensor   |          ❌           |   ✅ (client-side)   |      ❌       |   165 stars    |
| **EV Smart Charging**     | HACS integration |  Any sensor   |          ❌           |    ✅ (built-in)     |      ✅       |   279 stars    |
| **ENTSO-E**               | HACS integration |  ENTSO-E API  |  Partial (template)   |          ❌          |      ❌       |   259 stars    |
| **Tibber**                |       Core       |  Tibber API   | ✅ (Tibber customers) |          ❌          | ❌ (app only) |  2% installs   |
| **Our Spot Price API**    |  External REST   | Nord Pool API | **✅ (server-side)**  | **✅ (server-side)** |      ❌       |      New       |

---

## Topic 3: The "Cheapest Window" Scheduling Pattern

### Pattern A: Binary Sensor Approach (Recommended)

**How it works**: A binary sensor is ON during cheap hours, OFF otherwise. An automation triggers on state change to turn devices on/off.

```yaml
# Binary sensor: ON when inside cheapest window
template:
  - binary_sensor:
      - name: "Cheap Window Active"
        state: >
          {% set start = states('sensor.cheapest_window_start') %}
          {% set end = states('sensor.cheapest_window_end') %}
          {{ start <= now().isoformat() < end }}

# Automation: control device based on binary sensor
automation:
  - alias: "EV Charging - Cheap Hours"
    triggers:
      - trigger: state
        entity_id: binary_sensor.cheap_window_active
    actions:
      - action: switch.turn_{{ trigger.to_state.state }}
        target:
          entity_id: switch.ev_charger
```

**Pros**: Simple, visual (shows on/off in dashboard), works with any device, survives restarts.
**Cons**: Requires the cheapest window times to be pre-calculated and stored.

### Pattern B: Input Datetime + Time Trigger

**How it works**: Store cheapest start/end times in `input_datetime` helpers. Use time triggers that reference these helpers.

```yaml
input_datetime:
  cheap_start:
    has_date: true
    has_time: true
  cheap_end:
    has_date: true
    has_time: true

automation:
  - alias: "Start Charging at Cheap Time"
    triggers:
      - trigger: time
        at: input_datetime.cheap_start
    actions:
      - action: switch.turn_on
        target:
          entity_id: switch.ev_charger
  - alias: "Stop Charging at Cheap End"
    triggers:
      - trigger: time
        at: input_datetime.cheap_end
    actions:
      - action: switch.turn_off
        target:
          entity_id: switch.ev_charger
```

**Pros**: Precise timing, easy to understand, visible in dashboard.
**Cons**: Requires separate automation to update the input_datetime values. Two automations needed (start + stop).

### Pattern C: Price Threshold Condition

**How it works**: Check current price before allowing a device to run. No pre-calculation needed.

```yaml
automation:
  - alias: "Run Dishwasher When Cheap"
    triggers:
      - trigger: state
        entity_id: sensor.spot_price_total
    conditions:
      - condition: numeric_state
        entity_id: sensor.spot_price_total
        below: 10.0 # c/kWh threshold
    actions:
      - action: switch.turn_on
        target:
          entity_id: switch.dishwasher
```

**Pros**: Simplest — no window calculation needed.
**Cons**: Doesn't guarantee cheapest hours. Device may run during a "cheap enough" hour that isn't the cheapest. Doesn't handle contiguous windows.

### Pattern D: REST Binary Sensor (Direct from API)

**How it works**: A REST binary sensor that directly polls our API to determine if "now" is within the cheapest window.

```yaml
binary_sensor:
  - platform: rest
    name: "Cheapest Window Active"
    resource: "https://spot.calmdonut.com/api/v1/price/cheapest?duration=180"
    headers:
      Authorization: "Bearer YOUR_API_KEY"
    scan_interval: 900
    value_template: >
      {% set start = value_json.start %}
      {% set end = value_json.end %}
      {{ start <= utcnow().strftime('%Y-%m-%dT%H:%M:%S') + 'Z' < end }}
```

**Pros**: Single entity, minimal config, server-side calculation.
**Cons**: Polls API every 15 minutes. Relies on API availability. UTC comparison in Jinja2 can be tricky.

### Pattern E: Parameterized Script + Input Helpers (Hybrid UI/YAML)

**How it works**: A reusable HA script accepts a `duration` parameter, calls our REST API, and writes the cheapest window start/end times to `input_datetime` helpers. Other automations read those helpers to control devices. The key advantage: **only the REST command definition requires YAML** (~5 lines); everything else (helpers, script, automations) can be created entirely through the HA UI.

#### Layer 1: REST Command definition (YAML — one-time setup, ~5 lines)

```yaml
# configuration.yaml — the ONLY YAML needed
rest_command:
  get_cheapest_window:
    url: "https://spot.calmdonut.com/api/v1/price/cheapest?duration={{ duration }}"
    method: GET
    headers:
      Authorization: !secret spot_price_api_key
```

This calls our API which returns:

```json
{
  "start": "2026-02-27T22:00:00.000Z",
  "end": "2026-02-28T01:00:00.000Z",
  "startLocal": "2026-02-28T00:00:00.000+02:00",
  "endLocal": "2026-02-28T03:00:00.000+02:00",
  "averageTotalCentsKwh": 8.42,
  "prices": [ ... ]
}
```

The `response_variable` in HA makes this available as `result.content.startLocal`, `result.content.endLocal`, `result.content.averageTotalCentsKwh`, etc.

#### Layer 2: Input Helpers (created via HA UI — Settings > Helpers)

Create these helpers via the UI (Settings > Devices & services > Helpers > Create helper):

- `input_datetime.cheap_window_start` — type: Date and time
- `input_datetime.cheap_window_end` — type: Date and time
- `input_number.cheap_window_avg_price` — type: Number (min: 0, max: 100, step: 0.01, unit: c/kWh)

These are visible in the dashboard and editable by the user. No YAML needed.

#### Layer 3: Reusable Script (created via HA UI — Settings > Scripts)

A script with a `duration` field parameter that:

1. Calls `rest_command.get_cheapest_window` with the duration
2. Receives the API response via `response_variable: result`
3. Parses `result.content.startLocal`, `result.content.endLocal`, and `result.content.averageTotalCentsKwh`
4. Writes each value to the corresponding input helper entity

```yaml
# Can be created via UI script editor — shown as YAML for documentation
script:
  update_cheapest_window:
    alias: "Update Cheapest Window"
    description: "Fetch the cheapest electricity window from Spot Price API"
    fields:
      duration:
        name: Duration
        description: "Duration in minutes (e.g., 180 for 3 hours)"
        required: true
        selector:
          number:
            min: 15
            max: 1440
            step: 15
            unit_of_measurement: min
    sequence:
      # Step 1: Call the API — response lands in "result"
      - action: rest_command.get_cheapest_window
        data:
          duration: "{{ duration }}"
        response_variable: result

      # Step 2: Parse result.content.startLocal → set input_datetime
      # API returns "2026-02-28T00:00:00.000+02:00"
      # input_datetime.set_datetime expects "YYYY-MM-DD HH:MM:SS"
      # We slice [:19] to get "2026-02-28T00:00:00" then replace T with space
      - action: input_datetime.set_datetime
        target:
          entity_id: input_datetime.cheap_window_start
        data:
          datetime: "{{ result.content.startLocal[:19] | replace('T', ' ') }}"

      # Step 3: Parse result.content.endLocal → set input_datetime
      - action: input_datetime.set_datetime
        target:
          entity_id: input_datetime.cheap_window_end
        data:
          datetime: "{{ result.content.endLocal[:19] | replace('T', ' ') }}"

      # Step 4: Parse result.content.averageTotalCentsKwh → set input_number
      - action: input_number.set_value
        target:
          entity_id: input_number.cheap_window_avg_price
        data:
          value: "{{ result.content.averageTotalCentsKwh }}"
```

**How the response parsing works:**

| API response field                                              | Jinja2 template                                             | Result                  | Target entity                         |
| --------------------------------------------------------------- | ----------------------------------------------------------- | ----------------------- | ------------------------------------- |
| `result.content.startLocal` = `"2026-02-28T00:00:00.000+02:00"` | `{{ result.content.startLocal[:19] \| replace('T', ' ') }}` | `"2026-02-28 00:00:00"` | `input_datetime.cheap_window_start`   |
| `result.content.endLocal` = `"2026-02-28T03:00:00.000+02:00"`   | `{{ result.content.endLocal[:19] \| replace('T', ' ') }}`   | `"2026-02-28 03:00:00"` | `input_datetime.cheap_window_end`     |
| `result.content.averageTotalCentsKwh` = `8.42`                  | `{{ result.content.averageTotalCentsKwh }}`                 | `8.42`                  | `input_number.cheap_window_avg_price` |

The `[:19]` slice strips the timezone offset and milliseconds (e.g., `".000+02:00"`), and `| replace('T', ' ')` converts the ISO format to the `YYYY-MM-DD HH:MM:SS` format that `input_datetime.set_datetime` expects.

#### Layer 4: Automations (created via HA UI)

All of these can be built entirely in the HA automation editor (Settings > Automations & scenes > Create automation).

**Automation 1: Daily update** — triggers at 14:15 (after next-day prices are published) and calls the script:

```yaml
automation:
  - alias: "Update EV Charge Window Daily"
    triggers:
      - trigger: time
        at: "14:15:00"
    actions:
      - action: script.update_cheapest_window
        data:
          duration: 180
```

**Automation 2: Start charging** — triggers at the cheap window start time:

```yaml
automation:
  - alias: "Start EV Charging"
    triggers:
      - trigger: time
        at: input_datetime.cheap_window_start
    actions:
      - action: switch.turn_on
        target:
          entity_id: switch.ev_charger
```

**Automation 3: Stop charging** — triggers at the cheap window end time:

```yaml
automation:
  - alias: "Stop EV Charging"
    triggers:
      - trigger: time
        at: input_datetime.cheap_window_end
    actions:
      - action: switch.turn_off
        target:
          entity_id: switch.ev_charger
```

#### Feasibility Assessment

| Aspect                          | Assessment                                                                                               |
| ------------------------------- | -------------------------------------------------------------------------------------------------------- |
| **YAML required**               | Only `rest_command` definition (~5 lines). Everything else via UI.                                       |
| **Reusable**                    | ✅ — Same script works for EV charging, water heater, dishwasher — just pass different `duration`        |
| **Multiple consumers**          | ✅ — Any automation can read `input_datetime.cheap_window_start/end`                                     |
| **Dashboard visibility**        | ✅ — Input helpers show as cards: "Today's charge window: 01:00 – 04:00"                                 |
| **User can override**           | ✅ — User can manually edit the input_datetime values in the dashboard if they want to change the window |
| **Parameterized**               | ✅ — Script `fields` create typed parameter inputs visible in the UI when calling the script             |
| **Survives restart**            | ✅ — Input helpers restore state from database                                                           |
| **Precise timing**              | ✅ — `time` trigger on `input_datetime` fires at exact second, not on polling interval                   |
| **Error handling**              | ⚠️ — If API call fails, input_datetimes keep their old values (stale but safe)                           |
| **Complexity for average user** | Low-Medium — one YAML block to copy-paste, rest is UI clicks                                             |
| **Compared to Pattern D**       | More setup but more flexible, more visible, more reusable, and mostly UI-configurable                    |

**Key advantage over Pattern A/D**: The input_datetime helpers act as a **shared state** that multiple automations can read. You could have:

- One automation that charges the EV during the cheap window
- Another automation that runs the water heater during the same window
- A dashboard card showing "Next cheap window: 01:00 – 04:00 (avg 6.2 c/kWh)"
- A different script call with `duration: 60` for a 1-hour dishwasher window

**Evidence**: HA script docs confirm `fields` support with typed selectors (number, text, etc.) and full UI editor support. Used by 99% of installations. HA input_datetime docs confirm UI creation via Settings > Helpers and `input_datetime.set_datetime` action for programmatic updates. Used by 15.1% of installations. HA automation editor docs confirm time triggers can reference `input_datetime` entities. [Source: home-assistant.io/integrations/script/, home-assistant.io/integrations/input_datetime/, home-assistant.io/docs/automation/editor/]

### Recommendation: Pattern E (Script + Input Helpers) as Primary, Pattern D as Quick-Start

**Pattern E** (parameterized script + input helpers) is the **most practical recommendation** because:

1. **Mostly UI-configurable** — only ~5 lines of YAML to bootstrap the REST command
2. **Reusable** — one script serves multiple use cases (EV, heater, dishwasher) with different durations
3. **Visible** — input helpers show in dashboard, user can see and override the schedule
4. **Composable** — other automations can read the same helpers
5. **Precise timing** — `time` trigger on `input_datetime` fires at exact moment, not on polling interval
6. **Familiar pattern** — follows how HA community handles scheduled events (EV Smart Charging uses similar concepts)

**Pattern D** (REST binary sensor) is the **quick-start for simple cases** — 10 lines of YAML, no helpers or scripts needed, but less flexible and entirely YAML-based.

The EV Smart Charging HACS integration (279 stars) uses exactly this pattern — its `sensor.ev_smart_charging_charging` outputs "on"/"off" and users create automations that react to state changes.

The binary sensor approach is the most commonly recommended in the HA community because:

1. It creates a clear ON/OFF entity that any automation can use
2. It's visible in the dashboard
3. It works with any device type (switch, climate, cover, etc.)
4. State survives HA restarts
5. It decouples the "when is it cheap?" question from the "what to do?" action

The EV Smart Charging HACS integration (279 stars) uses exactly this pattern — its `sensor.ev_smart_charging_charging` outputs "on"/"off" and users create automations that react to state changes.

---

## Topic 4: Recommended Integration Architecture

### The Recommended Approach: REST Sensor + REST Binary Sensor

**For the average HA user**, the simplest end-to-end setup uses two built-in HA components with zero add-ons:

#### Step 1: REST Sensor for current price (monitoring)

```yaml
rest:
  - resource: "https://spot.calmdonut.com/api/v1/price/now"
    scan_interval: 900
    headers:
      Authorization: !secret spot_price_api_key
    sensor:
      - name: "Electricity Total Price"
        value_template: "{{ value_json.totalCentsKwh | round(2) }}"
        unit_of_measurement: "c/kWh"
        device_class: monetary
        state_class: measurement
        unique_id: spot_price_total
        json_attributes:
          - spotCentsKwh
          - marginCentsKwh
          - transferCentsKwh
          - isNightRate
          - deliveryStart
```

#### Step 2: REST Binary Sensor for cheapest window (scheduling)

```yaml
binary_sensor:
  - platform: rest
    name: "Cheapest 3h Window"
    resource: "https://spot.calmdonut.com/api/v1/price/cheapest?duration=180"
    headers:
      Authorization: !secret spot_price_api_key
    scan_interval: 900
    unique_id: spot_cheapest_3h
    device_class: power
    value_template: >
      {% set s = value_json.start %}
      {% set e = value_json.end %}
      {% set n = utcnow().strftime('%Y-%m-%dT%H:%M:%SZ') %}
      {{ s <= n < e }}
    json_attributes:
      - start
      - end
      - startLocal
      - endLocal
      - averageTotalCentsKwh
```

#### Step 3: Automation to control device

```yaml
automation:
  - alias: "EV Charging - Cheapest Hours"
    triggers:
      - trigger: state
        entity_id: binary_sensor.cheapest_3h_window
    actions:
      - action: "switch.turn_{{ trigger.to_state.state }}"
        target:
          entity_id: switch.ev_charger
```

### Why This Is Best

| Criterion                    | Assessment                                                                                              |
| ---------------------------- | ------------------------------------------------------------------------------------------------------- |
| **Easiest for average user** | ✅ — Copy-paste YAML, no add-ons, no Python                                                             |
| **Most robust**              | ✅ — Built-in HA components, state survives restarts, handles API errors (sensor becomes `unavailable`) |
| **Least maintenance**        | ✅ — No custom components to update, no HACS dependency                                                 |
| **Works with existing API**  | ✅ — Uses `/price/now` and `/price/cheapest` as-is                                                      |
| **Lines of YAML**            | ~35 lines total for full setup                                                                          |
| **Dashboard friendly**       | ✅ — Binary sensor shows on/off, price sensor shows current cost                                        |

### Alternative for Advanced Users: EV Smart Charging + Template Sensor

Users who want SOC-aware EV charging with more features should use the EV Smart Charging HACS integration with a template sensor that formats our API data into its generic price format. This is more complex but provides charge-level awareness, completion time targets, and non-continuous scheduling.

---

## Topic 5: Do We Need to Change Our API?

### Current API Assessment for HA Compatibility

| Aspect                                   | Assessment                                                                                 | Action Needed              |
| ---------------------------------------- | ------------------------------------------------------------------------------------------ | -------------------------- |
| **JSON response format**                 | ✅ Excellent — flat JSON objects, HA `value_template` and `json_attributes` work perfectly | None                       |
| **Field naming**                         | ✅ Good — camelCase is fine for `value_json.fieldName` access in Jinja2                    | None                       |
| **Auth method**                          | ✅ Good — Bearer token in `Authorization` header works with REST sensor `headers:`         | None                       |
| **`/price/now` for current price**       | ✅ Perfect — single object response, ideal for REST sensor                                 | None                       |
| **`/price/cheapest` for scheduling**     | ✅ Good — returns start/end times that can be compared with `utcnow()`                     | Minor improvement possible |
| **`/price/today` and `/price/tomorrow`** | ⚠️ Array response — HA REST sensor can't use arrays as state, only via attributes          | Document workaround        |
| **Polling vs. push**                     | ⚠️ Polling only — but this is fine for price data that changes every 15-60 minutes         | No change needed           |

### Potential API Improvements (Evaluated)

#### Idea 1: Add `/api/v1/price/cheapest/active` endpoint returning boolean

A dedicated endpoint that returns `{"active": true/false}` for whether the current time is within the cheapest window. This would simplify the binary sensor to:

```yaml
binary_sensor:
  - platform: rest
    resource: "https://spot.calmdonut.com/api/v1/price/cheapest/active?duration=180"
    value_template: "{{ value_json.active }}"
```

**Verdict**: **Nice to have, not essential.** The current `/price/cheapest` response already contains `start` and `end` times that can be compared in a `value_template`. The Jinja2 comparison is a bit verbose but works. Adding this endpoint would save ~3 lines of YAML per user.

#### Idea 2: Add webhook/push mechanism

**Verdict**: **Not needed.** Price data changes every 15-60 minutes. Polling at 15-minute intervals is 96 requests/day — well within our rate limits. HA's REST sensor handles this natively. Webhooks would add significant server complexity for minimal user benefit.

#### Idea 3: Adjust response format for HA

**Verdict**: **Not needed.** The current JSON format works perfectly with HA's `value_template` and `json_attributes`. No changes required.

#### Idea 4: Build a HACS custom component

**Verdict**: **Not recommended for MVP.** A HACS component would provide a nicer UI setup experience (config flow instead of YAML), but:

- Requires ongoing maintenance for HA version compatibility
- Must follow HA's integration quality guidelines
- The REST sensor approach works today with zero custom code
- Our user base is small (tens of users)
- The EV Smart Charging integration already supports generic price formats — users who need advanced features can use it

**Future consideration**: If the user base grows to hundreds and the YAML setup proves to be a support burden, a HACS component becomes worthwhile.

#### Idea 5: Add EV Smart Charging compatible price format endpoint

The EV Smart Charging integration supports a generic price format: `[{start, end, value}]`. We could add an endpoint that returns prices in this exact format, making integration trivial.

**Verdict**: **Worth considering as a future enhancement.** Our `/price/today` already returns an array of prices with `deliveryStart`, `deliveryEnd`, and `totalCentsKwh` — a template sensor can reformat this. But a dedicated endpoint would eliminate the template.

---

## Assumptions

| #   | Assumption                                                          | Risk if Wrong                                                                 |
| --- | ------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| A1  | HA users are comfortable editing `configuration.yaml`               | May need UI-only alternative (HACS component)                                 |
| A2  | 15-minute polling interval is sufficient for price-based automation | If 15-min MTU prices change more frequently, may need faster polling          |
| A3  | Bearer token auth in headers works reliably with HA REST sensor     | Verified in HA docs — low risk                                                |
| A4  | UTC timestamps in API responses can be reliably compared in Jinja2  | Jinja2 `utcnow()` formatting can be tricky — needs careful documentation      |
| A5  | Users will configure `secrets.yaml` for API key storage             | Some users may hardcode keys in YAML — security documentation needed          |
| A6  | The `/price/cheapest` endpoint recalculates on each call            | If cached, the binary sensor comparison remains valid within the cache window |

## Unknowns

| #   | Unknown                                                                               | Impact                                                        | Mitigation                                                    |
| --- | ------------------------------------------------------------------------------------- | ------------------------------------------------------------- | ------------------------------------------------------------- |
| U1  | How HA handles REST sensor when API returns error (500, 429)                          | Sensor becomes `unavailable` — automation may turn off device | Document `availability` template; test error behavior         |
| U2  | Whether `utcnow()` comparison with ISO 8601 strings works reliably across HA versions | Binary sensor may give wrong results                          | Test with actual HA instance; provide exact Jinja2 template   |
| U3  | Whether HA REST sensor caches responses across restarts                               | May cause stale cheapest-window data after restart            | Trigger-based template with startup trigger as alternative    |
| U4  | How many concurrent REST sensor polls our API can handle from multiple HA instances   | Rate limiting may block legitimate users                      | Current 60 req/min per API key is generous for 15-min polling |
| U5  | Whether EV Smart Charging's generic price format is stable across versions            | Template sensor may break on EV Smart Charging updates        | Pin to known working version; document format                 |

---

## Evidence Sufficiency Assessment

**Confidence: HIGH for proceeding to documentation planning.**

- All HA integration methods verified against current (2026.2.3) official documentation
- REST sensor, REST command, template sensor, and binary sensor capabilities confirmed with specific configuration options
- Existing integrations (Nord Pool, Cheapest Energy Hours, EV Smart Charging, ENTSO-E, Tibber) researched with current versions and feature sets
- The recommended approach (REST sensor + REST binary sensor) uses only built-in HA components — no custom code, no HACS dependency
- API response format confirmed compatible with HA's `value_template` and `json_attributes` parsing
- The binary sensor scheduling pattern is validated by the EV Smart Charging integration (279 stars) using the same approach

## Most Critical Unknown

**U2: UTC timestamp comparison in Jinja2.** The binary sensor that determines "is the cheapest window active right now?" depends on comparing UTC ISO 8601 strings from our API with `utcnow()` in Jinja2. String comparison of ISO 8601 timestamps works lexicographically, but edge cases around timezone formatting (trailing `Z` vs `+00:00`) could cause issues. This must be tested with an actual HA instance before publishing documentation.

---

## Summary of Recommendations

| Decision                        | Choice                                                               | Confidence |
| ------------------------------- | -------------------------------------------------------------------- | :--------: |
| Primary integration method      | REST Command + Script + Input Helpers (Pattern E — mostly UI)        |    High    |
| Quick-start alternative         | REST Binary Sensor (Pattern D — all YAML, simpler but less flexible) |    High    |
| Scheduling pattern              | Input datetime helpers read by time-trigger automations              |    High    |
| Documentation approach          | Copy-paste YAML examples in dedicated docs page + README link        |    High    |
| API changes needed              | None required; optional `/cheapest/active` endpoint is nice-to-have  |    High    |
| HACS component                  | Not recommended for MVP; reconsider if user base grows               |    High    |
| EV Smart Charging compatibility | Document template sensor bridge; no API change needed                |   Medium   |
| Webhook/push mechanism          | Not needed — polling at 15-min intervals is sufficient               |    High    |
| Recommended polling interval    | 900 seconds (15 minutes) for price sensors                           |    High    |
