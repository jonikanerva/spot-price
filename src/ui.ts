export const renderHomePage = (): string => `<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Spot Price</title>
    <style>
      :root {
        --bg: #f6f7f4;
        --card: #ffffff;
        --ink: #18221b;
        --accent: #2f855a;
        --muted: #5f6b62;
        --error: #b42318;
        --ok: #027a48;
      }
      * { box-sizing: border-box; }
      body {
        margin: 0;
        font-family: "IBM Plex Sans", "Avenir Next", sans-serif;
        color: var(--ink);
        background: radial-gradient(circle at 10% 10%, #ddeee2, var(--bg));
      }
      .wrap {
        max-width: 980px;
        margin: 40px auto;
        padding: 20px;
      }
      h1 { margin: 0 0 10px; font-size: 2rem; }
      p { color: var(--muted); }
      .grid {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
        gap: 16px;
        margin-top: 18px;
      }
      .card {
        background: var(--card);
        border-radius: 14px;
        padding: 16px;
        box-shadow: 0 8px 24px rgba(24, 34, 27, 0.08);
      }
      .actions {
        display: grid;
        grid-template-columns: 1fr;
        gap: 8px;
      }
      label { display: block; font-size: 0.9rem; margin: 10px 0 6px; }
      input {
        width: 100%;
        padding: 10px;
        border: 1px solid #ced7cf;
        border-radius: 10px;
      }
      button {
        margin-top: 12px;
        width: 100%;
        padding: 10px 12px;
        border: 0;
        border-radius: 10px;
        font-weight: 700;
        background: var(--accent);
        color: #fff;
        cursor: pointer;
      }
      button[disabled] {
        opacity: 0.65;
        cursor: not-allowed;
      }
      .status {
        margin-top: 8px;
        font-size: 12px;
        min-height: 18px;
      }
      .status.ok { color: var(--ok); }
      .status.err { color: var(--error); }
      #chart { margin-top: 14px; display: grid; gap: 6px; }
      .bar {
        height: 16px;
        border-radius: 6px;
        background: linear-gradient(90deg, #6bbf8f, #2f855a);
      }
      pre {
        white-space: pre-wrap;
        word-break: break-word;
        background: #f2f4ef;
        padding: 10px;
        border-radius: 8px;
        max-height: 220px;
        overflow: auto;
      }
      .hint {
        color: var(--muted);
        font-size: 12px;
        margin-top: 8px;
      }
      @media (max-width: 640px) {
        .wrap {
          margin: 20px auto;
          padding: 12px;
        }
        h1 {
          font-size: 1.5rem;
        }
        .card {
          padding: 12px;
        }
      }
    </style>
  </head>
  <body>
    <div class="wrap">
      <h1>Spot Price</h1>
      <p>Generate API key, view current total price, and find cheapest charging window.</p>

      <div class="grid">
        <section class="card">
          <h2>Create API key</h2>
          <label>User ID</label>
          <input id="userId" value="test-user" />
          <label>Key name</label>
          <input id="keyName" value="Home Assistant" />
          <button id="createKey">Create key</button>
          <div id="keyStatus" class="status"></div>
          <pre id="keyOutput"></pre>
        </section>

        <section class="card">
          <h2>Price now</h2>
          <label>API key</label>
          <input id="apiKey" placeholder="sp_..." />
          <div class="hint">API key is generated once and should be stored safely.</div>
          <button id="loadNow">Load current price</button>
          <div id="nowStatus" class="status"></div>
          <pre id="nowOutput"></pre>
        </section>

        <section class="card">
          <h2>Cheapest window</h2>
          <label>Duration (minutes)</label>
          <input id="duration" value="180" />
          <button id="loadCheapest">Find cheapest window</button>
          <div id="cheapestStatus" class="status"></div>
          <pre id="cheapestOutput"></pre>
        </section>

        <section class="card">
          <h2>Settings</h2>
          <label>Margin (c/kWh)</label>
          <input id="margin" value="0.45" />
          <label>Day transfer (c/kWh)</label>
          <input id="dayTransfer" value="3.02" />
          <label>Night transfer (c/kWh)</label>
          <input id="nightTransfer" value="1.55" />
          <label>Tax (c/kWh)</label>
          <input id="tax" value="2.79372" />
          <label>VAT (%)</label>
          <input id="vat" value="25.5" />
          <div class="actions">
            <button id="loadSettings">Load settings</button>
            <button id="saveSettings">Save settings</button>
          </div>
          <div id="settingsStatus" class="status"></div>
          <pre id="settingsOutput"></pre>
        </section>

        <section class="card" style="grid-column: 1 / -1;">
          <h2>Today's prices</h2>
          <button id="loadToday">Load chart</button>
          <div id="todayStatus" class="status"></div>
          <div id="chart"></div>
        </section>
      </div>
    </div>

    <script>
      const setText = (id, value) => {
        document.getElementById(id).textContent = typeof value === 'string' ? value : JSON.stringify(value, null, 2)
      }

      const setStatus = (id, kind, message) => {
        const el = document.getElementById(id)
        el.className = 'status ' + (kind || '')
        el.textContent = message || ''
      }

      const withLoading = async (buttonId, fn) => {
        const button = document.getElementById(buttonId)
        const original = button.textContent
        button.disabled = true
        button.textContent = 'Loading...'
        try {
          await fn()
        } finally {
          button.disabled = false
          button.textContent = original
        }
      }

      const requestJson = async (url, options = {}) => {
        const response = await fetch(url, options)
        let data = null
        try {
          data = await response.json()
        } catch {
          data = { error: 'Invalid JSON response' }
        }
        return { ok: response.ok, status: response.status, data }
      }

      const authHeaders = () => {
        const apiKey = document.getElementById('apiKey').value.trim()
        return apiKey ? { Authorization: 'Bearer ' + apiKey } : {}
      }

      document.getElementById('createKey').onclick = async () => withLoading('createKey', async () => {
        setStatus('keyStatus', '', '')
        const userId = document.getElementById('userId').value.trim()
        const name = document.getElementById('keyName').value.trim()
        if (!userId) {
          setStatus('keyStatus', 'err', 'userId is required')
          return
        }
        const result = await requestJson('/api/keys', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ userId, name })
        })
        const data = result.data
        setText('keyOutput', data)
        if (result.ok && data.apiKey) {
          document.getElementById('apiKey').value = data.apiKey
          setStatus('keyStatus', 'ok', 'API key created successfully')
        } else {
          setStatus('keyStatus', 'err', data.error || 'Failed to create API key')
        }
      })

      document.getElementById('loadNow').onclick = async () => withLoading('loadNow', async () => {
        setStatus('nowStatus', '', '')
        const result = await requestJson('/api/v1/price/now', { headers: authHeaders() })
        setText('nowOutput', result.data)
        setStatus('nowStatus', result.ok ? 'ok' : 'err', result.ok ? 'Current price loaded' : (result.data.error || 'Failed to load price'))
      })

      document.getElementById('loadCheapest').onclick = async () => withLoading('loadCheapest', async () => {
        setStatus('cheapestStatus', '', '')
        const duration = document.getElementById('duration').value.trim()
        const result = await requestJson('/api/v1/price/cheapest?duration=' + encodeURIComponent(duration), {
          headers: authHeaders()
        })
        setText('cheapestOutput', result.data)
        setStatus('cheapestStatus', result.ok ? 'ok' : 'err', result.ok ? 'Cheapest window loaded' : (result.data.error || 'Failed to load cheapest window'))
      })

      document.getElementById('loadToday').onclick = async () => withLoading('loadToday', async () => {
        setStatus('todayStatus', '', '')
        const chart = document.getElementById('chart')
        chart.innerHTML = ''
        const result = await requestJson('/api/v1/price/today', { headers: authHeaders() })
        const data = result.data
        if (!data.prices || !Array.isArray(data.prices)) {
          chart.textContent = data.error || 'No prices available'
          setStatus('todayStatus', 'err', data.error || 'No prices available')
          return
        }
        const max = Math.max(...data.prices.map(p => p.totalCentsKwh), 1)
        data.prices.forEach((p) => {
          const row = document.createElement('div')
          row.innerHTML = '<div style="font-size:12px;margin-bottom:4px">' + p.deliveryStart.slice(11, 16) + ' - ' + p.totalCentsKwh.toFixed(2) + ' c/kWh</div>'
          const bar = document.createElement('div')
          bar.className = 'bar'
          bar.style.width = Math.max(4, (p.totalCentsKwh / max) * 100) + '%'
          row.appendChild(bar)
          chart.appendChild(row)
        })
        setStatus('todayStatus', 'ok', 'Chart loaded')
      })

      document.getElementById('loadSettings').onclick = async () => withLoading('loadSettings', async () => {
        setStatus('settingsStatus', '', '')
        const result = await requestJson('/api/v1/settings', { headers: authHeaders() })
        const data = result.data
        setText('settingsOutput', data)
        if (data && !data.error) {
          document.getElementById('margin').value = String(data.marginCentsKwh)
          document.getElementById('dayTransfer').value = String(data.transferDayCentsKwh)
          document.getElementById('nightTransfer').value = String(data.transferNightCentsKwh)
          document.getElementById('tax').value = String(data.taxCentsKwh)
          document.getElementById('vat').value = String(data.vatPercent)
          setStatus('settingsStatus', 'ok', 'Settings loaded')
        } else {
          setStatus('settingsStatus', 'err', data.error || 'Failed to load settings')
        }
      })

      document.getElementById('saveSettings').onclick = async () => withLoading('saveSettings', async () => {
        setStatus('settingsStatus', '', '')
        const payload = {
          marginCentsKwh: Number(document.getElementById('margin').value),
          transferDayCentsKwh: Number(document.getElementById('dayTransfer').value),
          transferNightCentsKwh: Number(document.getElementById('nightTransfer').value),
          taxCentsKwh: Number(document.getElementById('tax').value),
          vatPercent: Number(document.getElementById('vat').value)
        }
        const result = await requestJson('/api/v1/settings', {
          method: 'PUT',
          headers: { ...authHeaders(), 'Content-Type': 'application/json' },
          body: JSON.stringify(payload)
        })
        setText('settingsOutput', result.data)
        setStatus('settingsStatus', result.ok ? 'ok' : 'err', result.ok ? 'Settings saved' : (result.data.error || 'Failed to save settings'))
      })
    </script>
  </body>
</html>`;
