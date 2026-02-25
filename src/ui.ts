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

      * { box-sizing: border-box; margin: 0; padding: 0; }

      body {
        font-family: "JetBrains Mono", "IBM Plex Mono", "Fira Code", monospace;
        color: var(--text);
        background:
          radial-gradient(ellipse at 10% 0%, #1f2a49 0%, transparent 40%),
          radial-gradient(ellipse at 90% 0%, #23194a 0%, transparent 30%),
          var(--bg);
        min-height: 100vh;
        line-height: 1.55;
      }

      .page {
        max-width: 1120px;
        margin: 0 auto;
        padding: 32px 20px 48px;
      }

      /* ---- cards ---- */
      .card {
        background: linear-gradient(180deg, var(--panel), var(--panel-2));
        border: 1px solid var(--border);
        border-radius: 14px;
        padding: 20px;
        box-shadow: 0 16px 48px rgba(5, 8, 16, 0.5);
      }

      /* ---- typography ---- */
      h1 { font-size: 1.65rem; letter-spacing: 0.3px; }
      h2 { font-size: 1rem; color: #d3def6; margin-bottom: 10px; }
      p { color: var(--muted); font-size: 0.85rem; }
      .muted { color: var(--muted); font-size: 12px; }

      /* ---- forms ---- */
      .auth-row {
        display: flex;
        gap: 10px;
        margin-top: 14px;
        flex-wrap: wrap;
      }
      .auth-row input { flex: 1; min-width: 140px; }

      input, select {
        border: 1px solid var(--border);
        background: #0f1528;
        color: var(--text);
        border-radius: 8px;
        padding: 10px 12px;
        font-family: inherit;
        font-size: 0.85rem;
        outline: none;
        transition: border-color 0.15s, box-shadow 0.15s;
      }
      input:focus {
        border-color: var(--accent);
        box-shadow: 0 0 0 2px rgba(93, 213, 255, 0.18);
      }

      /* ---- buttons ---- */
      .btn {
        border: 0;
        border-radius: 8px;
        padding: 10px 18px;
        font-family: inherit;
        font-size: 0.85rem;
        font-weight: 700;
        cursor: pointer;
        transition: opacity 0.15s, transform 0.1s;
        white-space: nowrap;
      }
      .btn:hover { opacity: 0.88; }
      .btn:active { transform: scale(0.97); }
      .btn[disabled] { opacity: 0.5; cursor: not-allowed; }

      .btn-primary {
        background: linear-gradient(135deg, var(--accent), var(--accent-2));
        color: #06101f;
      }
      .btn-secondary {
        background: rgba(255,255,255,0.04);
        color: var(--text);
        border: 1px solid var(--border);
      }
      .btn-secondary.active {
        border-color: var(--accent);
        color: var(--accent);
      }
      .btn-danger {
        background: rgba(255,107,138,0.12);
        color: var(--err);
        border: 1px solid rgba(255,107,138,0.25);
      }

      /* ---- status ---- */
      .status { min-height: 18px; font-size: 12px; margin-top: 8px; }
      .status.ok { color: var(--ok); }
      .status.err { color: var(--err); }

      /* ---- chart ---- */
      .chart-container {
        position: relative;
        margin-top: 12px;
      }
      .chart-box {
        border: 1px solid var(--border);
        border-radius: 10px;
        background: #0e1426;
        overflow: hidden;
        position: relative;
      }
      .chart-svg {
        display: block;
        width: 100%;
        height: 240px;
      }
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
        pointer-events: none;
        z-index: 10;
      }
      .legend {
        display: flex;
        gap: 16px;
        font-size: 12px;
        color: var(--muted);
        margin-top: 8px;
      }
      .dot {
        width: 10px; height: 10px;
        border-radius: 50%;
        display: inline-block;
        margin-right: 5px;
        vertical-align: middle;
      }

      /* ---- layout ---- */
      .landing { display: grid; gap: 18px; }
      .dashboard { display: none; }

      .topbar {
        display: flex;
        justify-content: space-between;
        align-items: center;
        margin-bottom: 18px;
        flex-wrap: wrap;
        gap: 10px;
      }
      .topbar-left { display: flex; align-items: baseline; gap: 12px; }
      .topbar-left p { margin: 0; }
      .nav { display: flex; gap: 6px; align-items: center; }
      .nav .username { color: var(--muted); font-size: 13px; margin-left: 8px; }

      .grid-2 {
        display: grid;
        grid-template-columns: minmax(0, 1fr) 320px;
        gap: 18px;
      }

      .settings-grid {
        display: grid;
        gap: 8px;
      }
      .settings-grid label {
        display: flex;
        justify-content: space-between;
        align-items: center;
        font-size: 13px;
        color: var(--muted);
        gap: 10px;
      }
      .settings-grid label input {
        width: 100px;
        text-align: right;
      }

      /* ---- api panel ---- */
      .api-panel { display: none; }

      .key-display {
        background: #0d1323;
        border: 1px solid var(--border);
        border-radius: 8px;
        padding: 12px 14px;
        font-size: 0.82rem;
        color: var(--accent);
        word-break: break-all;
        margin: 10px 0;
        font-family: inherit;
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

      /* ---- responsive ---- */
      @media (max-width: 980px) {
        .grid-2 { grid-template-columns: 1fr; }
        .auth-row { flex-direction: column; }
        .auth-row input { min-width: 0; }
      }
    </style>
  </head>
  <body>
    <main class="page">

      <!-- ===== LANDING (unauthenticated) ===== -->
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

      <!-- ===== DASHBOARD (authenticated) ===== -->
      <section id="dashboard" class="dashboard">
        <div class="topbar">
          <div class="topbar-left">
            <h1>Spot Price</h1>
          </div>
          <div class="nav">
            <button id="navDash" class="btn btn-secondary active">Dashboard</button>
            <button id="navApi" class="btn btn-secondary">API</button>
            <button id="logoutBtn" class="btn btn-secondary">Logout</button>
            <span id="usernameLabel" class="username"></span>
          </div>
        </div>

        <!-- dashboard view -->
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
              <button id="saveBtn" class="btn btn-primary" style="margin-top:6px">Save settings</button>
              <div id="settingsStatus" class="status"></div>
            </div>
          </aside>
        </div>

        <!-- api view -->
        <div id="apiView" class="api-panel">
          <article class="card">
            <h2>API</h2>
            <p>Use this key to call the REST API from Home Assistant or scripts.</p>

            <div class="key-display" id="apiKeyDisplay">Loading...</div>
            <button id="regenBtn" class="btn btn-danger">Regenerate API key</button>
            <div id="apiStatus" class="status"></div>

            <h2 style="margin-top:20px">Usage examples</h2>
            <pre id="apiExamples"></pre>
          </article>
        </div>
      </section>

    </main>

    <script>
      const $ = (id) => document.getElementById(id)

      /* ---- state ---- */
      const state = { session: null, apiKey: '' }

      /* ---- helpers ---- */
      const setStatus = (id, type, msg) => {
        const el = $(id)
        if (!el) return
        el.className = 'status ' + (type || '')
        el.textContent = msg || ''
      }

      const json = async (url, opts = {}) => {
        const res = await fetch(url, { credentials: 'include', ...opts })
        let data = null
        try { data = await res.json() } catch { data = { error: 'Invalid response' } }
        return { ok: res.ok, status: res.status, data }
      }

      const withLoading = async (btnId, fn) => {
        const b = $(btnId)
        const txt = b.textContent
        b.disabled = true
        b.textContent = '...'
        try { await fn() } finally { b.disabled = false; b.textContent = txt }
      }

      /* ---- SVG chart with axes + hover ---- */
      const CHART_PAD = { top: 14, right: 14, bottom: 28, left: 48 }

      const niceStep = (range) => {
        const raw = range / 4
        const mag = Math.pow(10, Math.floor(Math.log10(raw)))
        const options = [1, 2, 2.5, 5, 10]
        for (const m of options) { if (m * mag >= raw) return m * mag }
        return 10 * mag
      }

      const drawChart = (svgId, tooltipId, todayData, tomorrowData) => {
        const svg = $(svgId)
        const tooltip = $(tooltipId)
        if (!svg) return

        const W = svg.clientWidth || 800
        const H = svg.clientHeight || 240
        const pL = CHART_PAD.left, pR = CHART_PAD.right
        const pT = CHART_PAD.top, pB = CHART_PAD.bottom
        const cW = W - pL - pR
        const cH = H - pT - pB

        const todayVals = todayData.map(d => d.value)
        const tomorrowVals = tomorrowData.map(d => d.value)
        const all = [...todayVals, ...tomorrowVals]
        if (!all.length) { svg.innerHTML = ''; return }

        const dataMin = Math.min(...all)
        const dataMax = Math.max(...all)
        const range = Math.max(0.5, dataMax - dataMin)
        const step = niceStep(range)
        const yMin = Math.floor(dataMin / step) * step
        const yMax = Math.ceil(dataMax / step) * step
        const yRange = Math.max(0.001, yMax - yMin)

        let parts = []

        // grid lines + Y labels
        for (let v = yMin; v <= yMax + 0.001; v += step) {
          const y = pT + cH - ((v - yMin) / yRange) * cH
          parts.push('<line x1="'+pL+'" y1="'+y.toFixed(1)+'" x2="'+(W-pR)+'" y2="'+y.toFixed(1)+'" stroke="#1e2742" stroke-width="1"/>')
          parts.push('<text x="'+(pL-6)+'" y="'+(y+4).toFixed(1)+'" fill="#5a6a8a" font-size="11" text-anchor="end">'+v.toFixed(1)+'</text>')
        }

        // X labels
        const totalPoints = todayVals.length + tomorrowVals.length
        const hoursToLabel = todayVals.length > 0 && tomorrowVals.length > 0
          ? [0, 6, 12, 18, 24, 30, 36, 42, 48]
          : [0, 6, 12, 18, 24]
        const pointsPerHour = todayVals.length > 0 ? todayVals.length / 24 : 4
        hoursToLabel.forEach(h => {
          const idx = h * pointsPerHour
          if (idx > totalPoints) return
          const x = pL + (idx / Math.max(1, totalPoints - 1)) * cW
          if (x < pL - 5 || x > W - pR + 5) return
          const label = String(h >= 24 ? h - 24 : h).padStart(2, '0')
          parts.push('<text x="'+x.toFixed(1)+'" y="'+(H - 6)+'" fill="#5a6a8a" font-size="11" text-anchor="middle">'+label+'</text>')
        })

        // day separator
        if (tomorrowVals.length > 0 && todayVals.length > 0) {
          const sepX = pL + (todayVals.length / Math.max(1, totalPoints - 1)) * cW
          parts.push('<line x1="'+sepX.toFixed(1)+'" y1="'+pT+'" x2="'+sepX.toFixed(1)+'" y2="'+(H-pB)+'" stroke="#2b3655" stroke-width="1" stroke-dasharray="4,3"/>')
        }

        const makePath = (values, offset, color) => {
          if (!values.length) return ''
          const points = values.map((v, i) => {
            const x = pL + ((i + offset) / Math.max(1, totalPoints - 1)) * cW
            const y = pT + cH - ((v - yMin) / yRange) * cH
            return (i === 0 ? 'M' : 'L') + x.toFixed(2) + ' ' + y.toFixed(2)
          }).join(' ')
          return '<path d="'+points+'" fill="none" stroke="'+color+'" stroke-width="2" stroke-linejoin="round"/>'
        }

        parts.push(makePath(todayVals, 0, '#5dd5ff'))
        parts.push(makePath(tomorrowVals, todayVals.length, '#7b6bff'))

        // crosshair line (hidden initially)
        parts.push('<line id="'+svgId+'Cross" x1="0" y1="'+pT+'" x2="0" y2="'+(H-pB)+'" stroke="rgba(93,213,255,0.3)" stroke-width="1" visibility="hidden"/>')

        svg.setAttribute('viewBox', '0 0 ' + W + ' ' + H)
        svg.innerHTML = parts.join('')

        // hover interactivity
        const allData = [...todayData, ...tomorrowData]
        const crossLine = $(svgId + 'Cross')

        const handleHover = (clientX) => {
          const rect = svg.getBoundingClientRect()
          const mx = clientX - rect.left
          const ratio = (mx - pL) / cW
          const idx = Math.round(ratio * (allData.length - 1))
          if (idx < 0 || idx >= allData.length) {
            if (tooltip) tooltip.style.display = 'none'
            if (crossLine) crossLine.setAttribute('visibility', 'hidden')
            return
          }
          const pt = allData[idx]
          const x = pL + (idx / Math.max(1, allData.length - 1)) * cW
          if (crossLine) {
            crossLine.setAttribute('x1', x.toFixed(1))
            crossLine.setAttribute('x2', x.toFixed(1))
            crossLine.setAttribute('visibility', 'visible')
          }
          if (tooltip) {
            tooltip.textContent = pt.label + ' \\u2014 ' + pt.value.toFixed(2) + ' c/kWh'
            tooltip.style.display = 'block'
          }
        }

        svg.onmousemove = (e) => handleHover(e.clientX)
        svg.ontouchmove = (e) => { if (e.touches[0]) handleHover(e.touches[0].clientX) }
        svg.onmouseleave = () => {
          if (tooltip) tooltip.style.display = 'none'
          if (crossLine) crossLine.setAttribute('visibility', 'hidden')
        }
      }

      const toChartData = (prices, valueKey) =>
        prices.map(p => ({
          value: p[valueKey],
          label: new Date(p.deliveryStart).toLocaleTimeString('fi-FI', { hour: '2-digit', minute: '2-digit', hour12: false })
        }))

      /* ---- data loading ---- */
      const loadPublicChart = async () => {
        const r = await json('/api/public/spot')
        if (!r.ok) { setStatus('publicStatus', 'err', r.data.error || 'Failed'); return }
        const today = toChartData(r.data.today || [], 'spotCentsKwh')
        const tomorrow = toChartData(r.data.tomorrow || [], 'spotCentsKwh')
        drawChart('publicChart', 'publicTooltip', today, tomorrow)
        setStatus('publicStatus', 'ok', r.data.tomorrowAvailable ? 'Today + tomorrow loaded' : 'Tomorrow not yet available (published ~14:00 EET)')
      }

      const loadSession = async () => {
        const r = await json('/api/session')
        if (!r.ok || !r.data.session) { state.session = null; return false }
        state.session = r.data.session
        const name = r.data.username || r.data.session.user.name || 'user'
        $('usernameLabel').textContent = name
        return true
      }

      const loadSettings = async () => {
        const r = await json('/api/v1/me/settings')
        if (!r.ok) { setStatus('settingsStatus', 'err', r.data.error || 'Failed'); return }
        const s = r.data
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
        const r = await json('/api/v1/me/chart')
        if (!r.ok) { setStatus('totalStatus', 'err', r.data.error || 'Failed'); return }
        const today = toChartData(r.data.today || [], 'totalCentsKwh')
        const tomorrow = toChartData(r.data.tomorrow || [], 'totalCentsKwh')
        drawChart('totalChart', 'totalTooltip', today, tomorrow)
        setStatus('totalStatus', 'ok', r.data.tomorrowAvailable ? 'Today + tomorrow loaded' : 'Tomorrow not yet available')
      }

      const loadApiKey = async () => {
        const r = await json('/api/keys')
        if (!r.ok) { setStatus('apiStatus', 'err', r.data.error || 'Failed'); return }
        state.apiKey = r.data.apiKey
        $('apiKeyDisplay').textContent = state.apiKey
        renderExamples()
      }

      const renderExamples = () => {
        if (!state.apiKey) return
        const k = state.apiKey
        const base = location.origin
        $('apiExamples').innerHTML = [
          '<span class="comment"># Current total price</span>',
          'curl -sS \\\\',
          '  -H "Authorization: Bearer ' + k + '" \\\\',
          '  ' + base + '/api/v1/price/now',
          '',
          '<span class="comment"># Cheapest 3-hour window</span>',
          'curl -sS \\\\',
          '  -H "Authorization: Bearer ' + k + '" \\\\',
          '  ' + base + '/api/v1/price/cheapest?duration=180',
          '',
          '<span class="comment"># Today\\u2019s hourly prices</span>',
          'curl -sS \\\\',
          '  -H "Authorization: Bearer ' + k + '" \\\\',
          '  ' + base + '/api/v1/price/today',
          '',
          '<span class="comment"># Tomorrow\\u2019s prices (available after ~14:00 EET)</span>',
          'curl -sS \\\\',
          '  -H "Authorization: Bearer ' + k + '" \\\\',
          '  ' + base + '/api/v1/price/tomorrow'
        ].join('\\n')
      }

      /* ---- navigation ---- */
      const showDashboard = async () => {
        $('landing').style.display = 'none'
        $('dashboard').style.display = 'block'
        switchView('dash')
        await Promise.all([loadSettings(), loadTotalChart()])
      }

      const switchView = (view) => {
        const isDash = view === 'dash'
        $('dashView').style.display = isDash ? 'grid' : 'none'
        $('apiView').style.display = isDash ? 'none' : 'block'
        $('navDash').className = 'btn btn-secondary' + (isDash ? ' active' : '')
        $('navApi').className = 'btn btn-secondary' + (isDash ? '' : ' active')
        if (!isDash) loadApiKey()
      }

      /* ---- event handlers ---- */
      $('loginBtn').onclick = () => withLoading('loginBtn', async () => {
        const username = $('username').value.trim().toLowerCase()
        const password = $('password').value
        if (!username || !password) {
          setStatus('authStatus', 'err', 'Username and password required')
          return
        }
        const r = await json('/api/session/login-or-signup', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ username, password })
        })
        if (!r.ok) {
          setStatus('authStatus', 'err', r.data.message || r.data.error || 'Login failed')
          return
        }
        setStatus('authStatus', 'ok', 'Authenticated')
        await loadSession()
        await showDashboard()
      })

      $('logoutBtn').onclick = () => withLoading('logoutBtn', async () => {
        await json('/api/session/sign-out', { method: 'POST' })
        state.session = null
        state.apiKey = ''
        $('dashboard').style.display = 'none'
        $('landing').style.display = 'grid'
      })

      $('saveBtn').onclick = () => withLoading('saveBtn', async () => {
        const payload = {
          marginCentsKwh: Number($('margin').value),
          transferDayCentsKwh: Number($('dayTransfer').value),
          transferNightCentsKwh: Number($('nightTransfer').value),
          taxCentsKwh: Number($('tax').value),
          vatPercent: Number($('vat').value),
          nightStartHour: Number($('nightStart').value),
          nightEndHour: Number($('nightEnd').value)
        }
        const r = await json('/api/v1/me/settings', {
          method: 'PUT',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(payload)
        })
        if (!r.ok) { setStatus('settingsStatus', 'err', r.data.error || 'Save failed'); return }
        setStatus('settingsStatus', 'ok', 'Settings saved')
        await loadTotalChart()
      })

      $('navDash').onclick = () => switchView('dash')
      $('navApi').onclick = () => switchView('api')

      $('regenBtn').onclick = () => withLoading('regenBtn', async () => {
        const r = await json('/api/keys/regenerate', { method: 'POST' })
        if (!r.ok) { setStatus('apiStatus', 'err', r.data.error || 'Failed'); return }
        state.apiKey = r.data.apiKey
        $('apiKeyDisplay').textContent = state.apiKey
        renderExamples()
        setStatus('apiStatus', 'ok', 'New API key generated. Old key is now invalid.')
      })

      /* ---- init ---- */
      ;(async () => {
        await loadPublicChart()
        const has = await loadSession()
        if (has) await showDashboard()
      })()
    </script>
  </body>
</html>`;
