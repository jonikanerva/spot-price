import { DELIVERY_AREAS, SUPPORTED_TIMEZONES } from "./areas.js";
import { renderHomePageClientScript } from "./ui-client.js";

const renderAreaOptions = (): string =>
  DELIVERY_AREAS.map(
    (area) =>
      `<option value="${area.code}">${area.name} (${area.code})</option>`,
  ).join("\n");

const renderTimezoneOptions = (): string =>
  SUPPORTED_TIMEZONES.map(
    (timezone) => `<option value="${timezone}">${timezone}</option>`,
  ).join("\n");

const renderAreaTimezoneMap = (): string => {
  const entries = DELIVERY_AREAS.map(
    (area) => `${JSON.stringify(area.code)}: ${JSON.stringify(area.timezone)}`,
  );
  return `{ ${entries.join(", ")} }`;
};

export const renderHomePage = (): string => `<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Spot Price</title>
    <style>
      :root {
        --bg: #0b0f1a;
        --panel: #141b2f;
        --panel-2: #1a223a;
        --text: #e6edf7;
        --muted: #96a4c2;
        --border: #2b3655;
        --accent: #5dd5ff;
        --accent-2: #7b6bff;
        --ok: #41d39d;
        --err: #ff6b8a;
      }

      * { box-sizing: border-box; margin: 0; padding: 0; }
      body {
        font-family: "JetBrains Mono", "IBM Plex Mono", "Fira Code", monospace;
        color: var(--text);
        background: radial-gradient(ellipse at 10% 0%, #1f2a49 0%, transparent 40%), radial-gradient(ellipse at 90% 0%, #23194a 0%, transparent 30%), var(--bg);
        min-height: 100vh;
      }

      .page { max-width: 1120px; margin: 0 auto; padding: 32px 20px 48px; }
      .card {
        background: linear-gradient(180deg, var(--panel), var(--panel-2));
        border: 1px solid var(--border);
        border-radius: 14px;
        padding: 20px;
        box-shadow: 0 16px 48px rgba(5, 8, 16, 0.5);
      }
      h1 { font-size: 1.65rem; }
      h2 { font-size: 1rem; color: #d3def6; margin-bottom: 10px; }
      p { color: var(--muted); font-size: 0.85rem; }
      .muted { color: var(--muted); font-size: 12px; }

      .landing { display: grid; gap: 18px; }
      .dashboard { display: none; }
      .auth-row { display: flex; gap: 10px; margin-top: 14px; flex-wrap: wrap; }
      .auth-row input { flex: 1; min-width: 140px; }

      input, select {
        border: 1px solid var(--border);
        background: #0f1528;
        color: var(--text);
        border-radius: 8px;
        padding: 10px 12px;
        font-family: inherit;
        font-size: 0.85rem;
      }
      .btn {
        border: 0;
        border-radius: 8px;
        padding: 10px 18px;
        font-family: inherit;
        font-size: 0.85rem;
        font-weight: 700;
        cursor: pointer;
      }
      .btn-primary { background: linear-gradient(135deg, var(--accent), var(--accent-2)); color: #06101f; }
      .btn-secondary { background: rgba(255,255,255,0.04); color: var(--text); border: 1px solid var(--border); }
      .btn-secondary.active { border-color: var(--accent); color: var(--accent); }
      .btn-danger { background: rgba(255,107,138,0.12); color: var(--err); border: 1px solid rgba(255,107,138,0.25); }

      .status { min-height: 18px; font-size: 12px; margin-top: 8px; }
      .status.ok { color: var(--ok); }
      .status.err { color: var(--err); }

      .chart-container { position: relative; margin-top: 12px; }
      .chart-box { border: 1px solid var(--border); border-radius: 10px; background: #0e1426; overflow: hidden; }
      .chart-svg { display: block; width: 100%; height: 240px; }
      .chart-tooltip {
        display: none;
        position: absolute;
        top: 8px;
        right: 12px;
        background: rgba(11, 15, 26, 0.92);
        border: 1px solid var(--border);
        border-radius: 6px;
        padding: 6px 10px;
        font-size: 12px;
        color: var(--accent);
      }
      .legend { display: flex; gap: 16px; font-size: 12px; color: var(--muted); margin-top: 8px; }
      .dot { width: 10px; height: 10px; border-radius: 50%; display: inline-block; margin-right: 5px; vertical-align: middle; }

      .topbar { display: flex; justify-content: space-between; align-items: center; margin-bottom: 18px; flex-wrap: wrap; gap: 10px; }
      .nav { display: flex; gap: 6px; align-items: center; }
      .nav .username { color: var(--muted); font-size: 13px; margin-left: 8px; }
      .grid-2 { display: grid; grid-template-columns: minmax(0, 1fr) 320px; gap: 18px; }
      .settings-grid { display: grid; gap: 8px; }
      .settings-grid label { display: flex; justify-content: space-between; align-items: center; font-size: 13px; color: var(--muted); gap: 10px; }
      .settings-grid label.stacked { flex-direction: column; align-items: stretch; gap: 4px; }
      .settings-grid label input { width: 100px; text-align: right; }

      .api-panel { display: none; }
      .key-display-wrap { position: relative; margin: 10px 0; }
      .key-display {
        background: #0d1323;
        border: 1px solid var(--border);
        border-radius: 8px;
        padding: 12px 60px 12px 14px;
        font-size: 0.82rem;
        color: var(--accent);
        word-break: break-all;
      }
      pre {
        background: #0d1323;
        border: 1px solid var(--border);
        border-radius: 8px;
        padding: 14px;
        color: #c8d6f4;
        overflow: auto;
        white-space: pre-wrap;
        font-family: inherit;
        font-size: 0.8rem;
        line-height: 1.6;
        margin-top: 12px;
      }
      pre .comment { color: #5a6a8a; }
      .example-block { position: relative; margin-top: 12px; }
      .example-block pre { margin-top: 0; padding-right: 70px; }
      .copy-btn {
        position: absolute;
        top: 8px;
        right: 8px;
        background: rgba(255,255,255,0.06);
        border: 1px solid var(--border);
        color: var(--muted);
        border-radius: 6px;
        padding: 4px 10px;
        font-family: inherit;
        font-size: 11px;
        cursor: pointer;
      }
      .copy-btn.copied { color: var(--ok); border-color: var(--ok); }

      @media (max-width: 980px) {
        .grid-2 { grid-template-columns: 1fr; }
        .auth-row { flex-direction: column; }
        .auth-row input { min-width: 0; }
      }
    </style>
  </head>
  <body>
    <main class="page">
      <section id="landing" class="landing">
        <article class="card">
          <h1>Spot Price</h1>
          <p>Finnish spot electricity prices + total cost calculator for Home Assistant.</p>
          <div class="auth-row">
            <input id="username" placeholder="username" autocomplete="username" />
            <input id="password" type="password" placeholder="password" autocomplete="current-password" />
            <button id="loginBtn" class="btn btn-primary">Login or Signup</button>
          </div>
          <div id="authStatus" class="status"></div>
        </article>

        <article class="card">
          <h2>Spot price &mdash; today + tomorrow</h2>
          <p>15-min intervals, c/kWh. Source: Nord Pool.</p>
          <div class="chart-container">
            <div class="chart-box">
              <svg id="publicChart" class="chart-svg"></svg>
              <div id="publicTooltip" class="chart-tooltip"></div>
            </div>
            <div class="legend">
              <span><span class="dot" style="background:var(--accent)"></span>Today</span>
              <span><span class="dot" style="background:var(--accent-2)"></span>Tomorrow</span>
            </div>
          </div>
          <div id="publicStatus" class="status"></div>
        </article>
      </section>

      <section id="dashboard" class="dashboard">
        <div class="topbar">
          <h1>Spot Price</h1>
          <div class="nav">
            <button id="navDash" class="btn btn-secondary active">Dashboard</button>
            <button id="navApi" class="btn btn-secondary">API</button>
            <button id="logoutBtn" class="btn btn-secondary">Logout</button>
            <span id="usernameLabel" class="username"></span>
          </div>
        </div>

        <div id="dashView" class="grid-2">
          <article class="card">
            <h2>Total price (your contract)</h2>
            <p class="muted">15-min intervals, c/kWh incl. margin + transfer + tax + VAT</p>
            <div class="chart-container">
              <div class="chart-box">
                <svg id="totalChart" class="chart-svg"></svg>
                <div id="totalTooltip" class="chart-tooltip"></div>
              </div>
              <div class="legend">
                <span><span class="dot" style="background:var(--accent)"></span>Today</span>
                <span><span class="dot" style="background:var(--accent-2)"></span>Tomorrow</span>
              </div>
            </div>
            <div id="totalStatus" class="status"></div>
          </article>

          <aside class="card">
            <h2>Settings</h2>
            <div class="settings-grid">
              <label>Margin c/kWh <input id="margin" type="number" step="0.01" /></label>
              <label>Day transfer c/kWh <input id="dayTransfer" type="number" step="0.01" /></label>
              <label>Night transfer c/kWh <input id="nightTransfer" type="number" step="0.01" /></label>
              <label>Tax c/kWh <input id="tax" type="number" step="0.01" /></label>
              <label>VAT % <input id="vat" type="number" step="0.1" /></label>
              <label>Night start hour <input id="nightStart" type="number" min="0" max="23" /></label>
              <label>Night end hour <input id="nightEnd" type="number" min="0" max="23" /></label>
              <label class="stacked">Delivery area
                <select id="area">
                  ${renderAreaOptions()}
                </select>
              </label>
              <label class="stacked">Timezone
                <select id="timezone">
                  ${renderTimezoneOptions()}
                </select>
              </label>
              <button id="saveBtn" class="btn btn-primary" style="margin-top:6px">Save settings</button>
              <div id="settingsStatus" class="status"></div>
            </div>
          </aside>
        </div>

        <div id="apiView" class="api-panel">
          <article class="card">
            <h2>API</h2>
            <p>Use this key to call the REST API from Home Assistant or scripts.</p>

            <div class="key-display-wrap">
              <div class="key-display" id="apiKeyDisplay">Loading...</div>
              <button class="copy-btn" id="copyKeyBtn">Copy</button>
            </div>
            <button id="regenBtn" class="btn btn-danger">Regenerate API key</button>
            <div id="apiStatus" class="status"></div>

            <h2 style="margin-top:20px">Usage examples</h2>
            <div id="apiExamples"></div>

            <h2 style="margin-top:28px">Home Assistant</h2>
            <p style="margin-bottom:4px">Drop-in REST commands for Home Assistant. Copy this YAML to your HA config directory as <code>spot-price.yaml</code>, then add <code>rest_command: !include spot-price.yaml</code> to <code>configuration.yaml</code> and restart HA.</p>
            <div id="haYamlBlock" class="example-block">
              <pre id="haYamlContent" style="max-height:320px;overflow-y:auto">Loading...</pre>
              <button class="copy-btn" id="copyHaYamlBtn">Copy</button>
            </div>

            <h2 style="margin-top:28px">API reference</h2>
            <p style="margin-bottom:12px">Full request/response schemas with interactive examples:</p>
            <a href="/api/docs" target="_blank" class="btn btn-secondary" style="display:inline-block;margin-top:8px;text-decoration:none">
              Open interactive API docs &rarr;
            </a>
          </article>
        </div>
      </section>
    </main>

    <script>${renderHomePageClientScript(renderAreaTimezoneMap())}</script>
  </body>
</html>`;
