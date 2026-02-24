export const renderHomePage = (): string => `<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Spot Price</title>
    <style>
      :root {
        --bg: #0b0f1a;
        --bg-soft: #111729;
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

      * { box-sizing: border-box; }
      body {
        margin: 0;
        font-family: "JetBrains Mono", "IBM Plex Sans", sans-serif;
        color: var(--text);
        background:
          radial-gradient(circle at 10% 5%, #1f2a49 0%, transparent 30%),
          radial-gradient(circle at 90% 0%, #23194a 0%, transparent 24%),
          var(--bg);
      }

      .page {
        max-width: 1180px;
        margin: 0 auto;
        padding: 28px 18px 40px;
      }

      .card {
        background: linear-gradient(180deg, var(--panel), var(--panel-2));
        border: 1px solid var(--border);
        border-radius: 14px;
        padding: 16px;
        box-shadow: 0 16px 40px rgba(5, 8, 16, 0.45);
      }

      h1 { margin: 0; font-size: 1.65rem; letter-spacing: 0.2px; }
      h2 { margin: 0 0 10px; font-size: 1rem; color: #d3def6; }
      p { margin: 8px 0 0; color: var(--muted); font-size: 0.9rem; }

      .landing {
        display: grid;
        gap: 16px;
      }

      .auth-form {
        display: grid;
        grid-template-columns: 1fr 1fr auto;
        gap: 10px;
        margin-top: 12px;
      }

      input {
        width: 100%;
        border: 1px solid var(--border);
        background: #0f1528;
        color: var(--text);
        border-radius: 10px;
        padding: 11px;
        outline: none;
      }

      input:focus {
        border-color: var(--accent);
        box-shadow: 0 0 0 2px rgba(93, 213, 255, 0.15);
      }

      button {
        border: 0;
        border-radius: 10px;
        padding: 11px 14px;
        background: linear-gradient(90deg, var(--accent), var(--accent-2));
        color: #06101f;
        font-weight: 700;
        cursor: pointer;
      }

      button.secondary {
        background: #0f1528;
        color: var(--text);
        border: 1px solid var(--border);
      }

      button[disabled] { opacity: 0.65; cursor: not-allowed; }

      .status { min-height: 18px; font-size: 12px; margin-top: 10px; }
      .status.ok { color: var(--ok); }
      .status.err { color: var(--err); }

      .chart-wrap { margin-top: 10px; }
      .legend {
        display: flex;
        gap: 14px;
        font-size: 12px;
        color: var(--muted);
        margin-bottom: 10px;
      }
      .dot {
        width: 10px;
        height: 10px;
        border-radius: 50%;
        display: inline-block;
        margin-right: 6px;
      }

      .line-chart {
        position: relative;
        height: 220px;
        border: 1px solid var(--border);
        border-radius: 10px;
        background: #0e1426;
        overflow: hidden;
      }

      .line {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 100%;
      }

      .dashboard {
        display: none;
      }

      .topbar {
        display: flex;
        justify-content: space-between;
        align-items: center;
        gap: 10px;
        margin-bottom: 14px;
      }

      .nav { display: flex; gap: 8px; }

      .layout {
        display: grid;
        grid-template-columns: minmax(0, 1fr) 340px;
        gap: 16px;
      }

      .settings-grid { display: grid; gap: 8px; }
      .api-panel { display: none; margin-top: 10px; }

      pre {
        background: #0d1323;
        border: 1px solid var(--border);
        border-radius: 10px;
        padding: 10px;
        color: #c8d6f4;
        overflow: auto;
        white-space: pre-wrap;
      }

      .muted { color: var(--muted); font-size: 12px; }

      @media (max-width: 980px) {
        .auth-form {
          grid-template-columns: 1fr;
        }

        .layout {
          grid-template-columns: 1fr;
        }
      }
    </style>
  </head>
  <body>
    <main class="page">
      <section id="landing" class="landing">
        <article class="card">
          <h1>Spot Price</h1>
          <p>Login with username + password. If user does not exist, it will be created automatically.</p>
          <div class="auth-form">
            <input id="username" placeholder="username (a-z0-9_-)">
            <input id="password" type="password" placeholder="password">
            <button id="loginOrSignup">Login or Signup</button>
          </div>
          <div id="authStatus" class="status"></div>
        </article>

        <article class="card">
          <h2>Spot price (today + tomorrow)</h2>
          <p>Public view, 15-min intervals, c/kWh.</p>
          <div class="chart-wrap">
            <div class="legend">
              <span><span class="dot" style="background:#5dd5ff"></span>Today</span>
              <span><span class="dot" style="background:#8a7dff"></span>Tomorrow</span>
            </div>
            <div class="line-chart"><svg id="publicChart" class="line"></svg></div>
          </div>
          <div id="publicStatus" class="status"></div>
        </article>
      </section>

      <section id="dashboard" class="dashboard">
        <div class="topbar">
          <div>
            <h1>Dashboard</h1>
            <p id="sessionLabel">Logged in</p>
          </div>
          <div class="nav">
            <button id="navDashboard" class="secondary">Dashboard</button>
            <button id="navApi" class="secondary">API</button>
            <button id="logout" class="secondary">Logout</button>
          </div>
        </div>

        <div class="layout">
          <article class="card">
            <h2>Total price chart (by your settings)</h2>
            <p class="muted">15-min intervals in c/kWh</p>
            <div class="line-chart"><svg id="totalChart" class="line"></svg></div>
            <div id="totalStatus" class="status"></div>

            <section id="apiPanel" class="api-panel">
              <h2 style="margin-top:14px">API</h2>
              <p class="muted">Generate key and call REST API from Home Assistant or scripts.</p>
              <div style="display:flex; gap:8px; margin:8px 0">
                <input id="apiKeyName" placeholder="API key name" value="Home Assistant" />
                <button id="createKey">Create key</button>
              </div>
              <div id="apiStatus" class="status"></div>
              <pre id="apiOutput"></pre>
              <pre id="apiExamples"></pre>
            </section>
          </article>

          <aside class="card">
            <h2>Settings</h2>
            <div class="settings-grid">
              <input id="margin" placeholder="Margin c/kWh" />
              <input id="dayTransfer" placeholder="Day transfer c/kWh" />
              <input id="nightTransfer" placeholder="Night transfer c/kWh" />
              <input id="tax" placeholder="Tax c/kWh" />
              <input id="vat" placeholder="VAT %" />
              <input id="nightStart" placeholder="Night start hour" />
              <input id="nightEnd" placeholder="Night end hour" />
              <button id="saveSettings">Save settings</button>
              <div id="settingsStatus" class="status"></div>
            </div>
          </aside>
        </div>
      </section>
    </main>

    <script>
      const $ = (id) => document.getElementById(id)

      const state = {
        apiKey: '',
        session: null,
      }

      const setStatus = (id, type, msg) => {
        const el = $(id)
        el.className = 'status ' + (type || '')
        el.textContent = msg || ''
      }

      const requestJson = async (url, options = {}) => {
        const res = await fetch(url, { credentials: 'include', ...options })
        let data = null
        try { data = await res.json() } catch { data = { error: 'Invalid JSON' } }
        return { ok: res.ok, status: res.status, data }
      }

      const withLoading = async (buttonId, fn) => {
        const button = $(buttonId)
        const txt = button.textContent
        button.disabled = true
        button.textContent = '...'
        try { await fn() } finally { button.disabled = false; button.textContent = txt }
      }

      const linePath = (points, width, height, min, max) => {
        if (!points.length) return ''
        const safeRange = Math.max(0.0001, max - min)
        return points.map((v, i) => {
          const x = (i / Math.max(1, points.length - 1)) * width
          const y = height - ((v - min) / safeRange) * height
          return (i === 0 ? 'M' : 'L') + x.toFixed(2) + ' ' + y.toFixed(2)
        }).join(' ')
      }

      const drawChart = (svgId, todayValues, tomorrowValues) => {
        const svg = $(svgId)
        const width = svg.clientWidth || 800
        const height = svg.clientHeight || 220
        const all = [...todayValues, ...tomorrowValues]
        if (!all.length) {
          svg.innerHTML = ''
          return
        }
        const min = Math.min(...all)
        const max = Math.max(...all)
        const tPath = linePath(todayValues, width, height, min, max)
        const tmPath = linePath(tomorrowValues, width, height, min, max)

        svg.setAttribute('viewBox', '0 0 ' + width + ' ' + height)
        svg.innerHTML = [
          '<path d="' + tPath + '" fill="none" stroke="#5dd5ff" stroke-width="2"/>',
          tmPath ? '<path d="' + tmPath + '" fill="none" stroke="#8a7dff" stroke-width="2"/>' : ''
        ].join('')
      }

      const loadPublicChart = async () => {
        const result = await requestJson('/api/public/spot')
        if (!result.ok) {
          setStatus('publicStatus', 'err', result.data.error || 'Failed to load public chart')
          return
        }
        const today = (result.data.today || []).map(p => p.spotCentsKwh)
        const tomorrow = (result.data.tomorrow || []).map(p => p.spotCentsKwh)
        drawChart('publicChart', today, tomorrow)
        setStatus('publicStatus', 'ok', result.data.tomorrowAvailable ? 'Today + tomorrow loaded' : 'Tomorrow not available yet')
      }

      const loadSession = async () => {
        const result = await requestJson('/api/session')
        if (!result.ok || !result.data.session) {
          state.session = null
          return false
        }
        state.session = result.data.session
        const username = result.data.username || result.data.session.user.name || 'user'
        $('sessionLabel').textContent = 'Logged in as ' + username
        return true
      }

      const loadSettings = async () => {
        const result = await requestJson('/api/v1/me/settings')
        if (!result.ok) {
          setStatus('settingsStatus', 'err', result.data.error || 'Failed to load settings')
          return
        }
        const s = result.data
        $('margin').value = s.marginCentsKwh
        $('dayTransfer').value = s.transferDayCentsKwh
        $('nightTransfer').value = s.transferNightCentsKwh
        $('tax').value = s.taxCentsKwh
        $('vat').value = s.vatPercent
        $('nightStart').value = s.nightStartHour
        $('nightEnd').value = s.nightEndHour
        setStatus('settingsStatus', 'ok', 'Settings loaded')
      }

      const loadTotalChart = async () => {
        const result = await requestJson('/api/v1/me/chart')
        if (!result.ok) {
          setStatus('totalStatus', 'err', result.data.error || 'Failed to load total chart')
          return
        }
        const today = (result.data.today || []).map(p => p.totalCentsKwh)
        const tomorrow = (result.data.tomorrow || []).map(p => p.totalCentsKwh)
        drawChart('totalChart', today, tomorrow)
        setStatus('totalStatus', 'ok', result.data.tomorrowAvailable ? 'Today + tomorrow loaded' : 'Tomorrow not available yet')
      }

      const renderApiExamples = () => {
        if (!state.apiKey) return
        $('apiExamples').textContent = [
          'curl -sS "https://spot.calmdonut.com/api/v1/price/now" -H "Authorization: Bearer ' + state.apiKey + '"',
          'curl -sS "https://spot.calmdonut.com/api/v1/price/cheapest?duration=180" -H "Authorization: Bearer ' + state.apiKey + '"',
          'curl -sS "https://spot.calmdonut.com/api/v1/price/today" -H "Authorization: Bearer ' + state.apiKey + '"'
        ].join('\n\n')
      }

      const showDashboard = async () => {
        $('landing').style.display = 'none'
        $('dashboard').style.display = 'block'
        await loadSettings()
        await loadTotalChart()
      }

      $('loginOrSignup').onclick = async () => withLoading('loginOrSignup', async () => {
        const username = $('username').value.trim().toLowerCase()
        const password = $('password').value
        const result = await requestJson('/api/session/login-or-signup', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ username, password })
        })

        if (!result.ok) {
          setStatus('authStatus', 'err', result.data.message || result.data.error || 'Login failed')
          return
        }
        setStatus('authStatus', 'ok', 'Authenticated')
        await loadSession()
        await showDashboard()
      })

      $('logout').onclick = async () => withLoading('logout', async () => {
        await requestJson('/api/session/sign-out', { method: 'POST' })
        state.session = null
        state.apiKey = ''
        $('dashboard').style.display = 'none'
        $('landing').style.display = 'grid'
      })

      $('saveSettings').onclick = async () => withLoading('saveSettings', async () => {
        const payload = {
          marginCentsKwh: Number($('margin').value),
          transferDayCentsKwh: Number($('dayTransfer').value),
          transferNightCentsKwh: Number($('nightTransfer').value),
          taxCentsKwh: Number($('tax').value),
          vatPercent: Number($('vat').value),
          nightStartHour: Number($('nightStart').value),
          nightEndHour: Number($('nightEnd').value)
        }
        const result = await requestJson('/api/v1/me/settings', {
          method: 'PUT',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(payload)
        })
        if (!result.ok) {
          setStatus('settingsStatus', 'err', result.data.error || 'Failed to save')
          return
        }
        setStatus('settingsStatus', 'ok', 'Settings saved')
        await loadTotalChart()
      })

      $('navDashboard').onclick = () => {
        $('apiPanel').style.display = 'none'
      }

      $('navApi').onclick = () => {
        $('apiPanel').style.display = 'block'
      }

      $('createKey').onclick = async () => withLoading('createKey', async () => {
        const name = $('apiKeyName').value.trim() || 'Default'
        const result = await requestJson('/api/keys', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ name })
        })
        $('apiOutput').textContent = JSON.stringify(result.data, null, 2)
        if (!result.ok) {
          setStatus('apiStatus', 'err', result.data.error || 'Failed to create key')
          return
        }
        state.apiKey = result.data.apiKey || ''
        setStatus('apiStatus', 'ok', 'API key created')
        renderApiExamples()
      })

      ;(async () => {
        await loadPublicChart()
        const hasSession = await loadSession()
        if (hasSession) {
          await showDashboard()
        }
      })()
    </script>
  </body>
</html>`;
