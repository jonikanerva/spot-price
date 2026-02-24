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
          <pre id="keyOutput"></pre>
        </section>

        <section class="card">
          <h2>Price now</h2>
          <label>API key</label>
          <input id="apiKey" placeholder="sp_..." />
          <button id="loadNow">Load current price</button>
          <pre id="nowOutput"></pre>
        </section>

        <section class="card">
          <h2>Cheapest window</h2>
          <label>Duration (minutes)</label>
          <input id="duration" value="180" />
          <button id="loadCheapest">Find cheapest window</button>
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
          <button id="loadSettings">Load settings</button>
          <button id="saveSettings">Save settings</button>
          <pre id="settingsOutput"></pre>
        </section>

        <section class="card" style="grid-column: 1 / -1;">
          <h2>Today's prices</h2>
          <button id="loadToday">Load chart</button>
          <div id="chart"></div>
        </section>
      </div>
    </div>

    <script>
      const setText = (id, value) => {
        document.getElementById(id).textContent = typeof value === 'string' ? value : JSON.stringify(value, null, 2)
      }

      const authHeaders = () => {
        const apiKey = document.getElementById('apiKey').value.trim()
        return apiKey ? { Authorization: 'Bearer ' + apiKey } : {}
      }

      document.getElementById('createKey').onclick = async () => {
        const userId = document.getElementById('userId').value.trim()
        const name = document.getElementById('keyName').value.trim()
        const res = await fetch('/api/keys', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ userId, name })
        })
        const data = await res.json()
        setText('keyOutput', data)
        if (data.apiKey) {
          document.getElementById('apiKey').value = data.apiKey
        }
      }

      document.getElementById('loadNow').onclick = async () => {
        const res = await fetch('/api/v1/price/now', { headers: authHeaders() })
        setText('nowOutput', await res.json())
      }

      document.getElementById('loadCheapest').onclick = async () => {
        const duration = document.getElementById('duration').value.trim()
        const res = await fetch('/api/v1/price/cheapest?duration=' + encodeURIComponent(duration), {
          headers: authHeaders()
        })
        setText('cheapestOutput', await res.json())
      }

      document.getElementById('loadToday').onclick = async () => {
        const chart = document.getElementById('chart')
        chart.innerHTML = ''
        const res = await fetch('/api/v1/price/today', { headers: authHeaders() })
        const data = await res.json()
        if (!data.prices || !Array.isArray(data.prices)) {
          chart.textContent = 'No prices available'
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
      }

      document.getElementById('loadSettings').onclick = async () => {
        const res = await fetch('/api/v1/settings', { headers: authHeaders() })
        const data = await res.json()
        setText('settingsOutput', data)
        if (data && !data.error) {
          document.getElementById('margin').value = String(data.marginCentsKwh)
          document.getElementById('dayTransfer').value = String(data.transferDayCentsKwh)
          document.getElementById('nightTransfer').value = String(data.transferNightCentsKwh)
          document.getElementById('tax').value = String(data.taxCentsKwh)
          document.getElementById('vat').value = String(data.vatPercent)
        }
      }

      document.getElementById('saveSettings').onclick = async () => {
        const payload = {
          marginCentsKwh: Number(document.getElementById('margin').value),
          transferDayCentsKwh: Number(document.getElementById('dayTransfer').value),
          transferNightCentsKwh: Number(document.getElementById('nightTransfer').value),
          taxCentsKwh: Number(document.getElementById('tax').value),
          vatPercent: Number(document.getElementById('vat').value)
        }
        const res = await fetch('/api/v1/settings', {
          method: 'PUT',
          headers: { ...authHeaders(), 'Content-Type': 'application/json' },
          body: JSON.stringify(payload)
        })
        setText('settingsOutput', await res.json())
      }
    </script>
  </body>
</html>`;
