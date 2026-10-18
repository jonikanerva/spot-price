
      const $ = (id) => document.getElementById(id)

      const state = { session: null, apiKey: '' }
      const areaTimezoneMap = {{AREA_TIMEZONE_MAP}}

      const setStatus = (id, type, msg) => {
        const el = $(id)
        if (!el) return
        el.className = 'status ' + (type || '')
        el.textContent = msg || ''
      }

      const json = async (url, opts = {}) => {
        const res = await fetch(url, { credentials: 'include', ...opts })
        let data = null
        try {
          data = await res.json()
        } catch {
          data = { error: 'Invalid response' }
        }
        return { ok: res.ok, status: res.status, data }
      }

      const withLoading = async (buttonId, run) => {
        const button = $(buttonId)
        if (!button) return
        const initialText = button.textContent
        button.disabled = true
        button.textContent = 'Working...'
        try {
          await run()
        } finally {
          button.disabled = false
          button.textContent = initialText
        }
      }

      const markCopied = (button) => {
        button.textContent = 'Copied!'
        button.classList.add('copied')
        setTimeout(() => {
          button.textContent = 'Copy'
          button.classList.remove('copied')
        }, 1500)
      }

      const copyWithFeedback = async (button, text) => {
        await navigator.clipboard.writeText(text)
        markCopied(button)
      }

      // HH:MM of an ISO instant in timeZone, not in the time zone of the browser.
      const timeInZone = (timeZone) => {
        const format = new Intl.DateTimeFormat('fi-FI', {
          timeZone,
          hour: '2-digit',
          minute: '2-digit',
          hourCycle: 'h23',
        })
        return (iso) => {
          const parts = format.formatToParts(new Date(iso))
          const part = (type) => (parts.find((item) => item.type === type) || { value: '00' }).value
          return part('hour') + ':' + part('minute')
        }
      }

      // HH:MM of a local time text with an offset, such as localStart.
      const wallClock = (text) => {
        const match = /T(\d\d):(\d\d)/.exec(text || '')
        return match ? match[1] + ':' + match[2] : ''
      }

      const toChartData = (prices, valueKey, startLabel, endLabel) =>
        prices.map((price) => ({
          value: Number(price[valueKey]),
          start: Date.parse(price.deliveryStart),
          end: Date.parse(price.deliveryEnd),
          label: startLabel(price),
          endLabel: endLabel(price),
        }))

      const drawChart = (svgId, tooltipId, todayData, tomorrowData) => {
        const svg = $(svgId)
        const tooltip = $(tooltipId)
        if (!svg) return

        const points = [...todayData, ...tomorrowData]
        if (points.length === 0) {
          svg.innerHTML = '<text x="50%" y="50%" dominant-baseline="middle" text-anchor="middle" fill="#96a4c2" font-size="12">No data</text>'
          return
        }

        const width = svg.clientWidth || 760
        const height = 240
        const paddingLeft = 44
        const paddingRight = 10
        const paddingTop = 10
        const paddingBottom = 24
        const chartWidth = width - paddingLeft - paddingRight
        const chartHeight = height - paddingTop - paddingBottom

        const values = points.map((point) => point.value)
        const minValue = Math.min(...values)
        const maxValue = Math.max(...values)
        const span = Math.max(maxValue - minValue, 0.001)
        const yMin = Math.floor((minValue - span * 0.08) * 10) / 10
        const yMax = Math.ceil((maxValue + span * 0.12) * 10) / 10
        const ySpan = Math.max(yMax - yMin, 0.001)

        // The x axis is time: one local day is 23, 24, or 25 hours long.
        const domainStart = points[0].start
        const domainEnd = points[points.length - 1].end
        const domainSpan = Math.max(domainEnd - domainStart, 1)
        const toX = (time) =>
          paddingLeft + ((time - domainStart) / domainSpan) * chartWidth
        const toY = (value) =>
          paddingTop + (1 - (value - yMin) / ySpan) * chartHeight

        const pathFor = (series, color) => {
          if (series.length === 0) return ''
          const d = series
            .map((point, i) => {
              const cmd = i === 0 ? 'M' : 'L'
              return cmd + toX(point.start).toFixed(1) + ' ' + toY(point.value).toFixed(1)
            })
            .join(' ')
          return '<path d="' + d + '" fill="none" stroke="' + color + '" stroke-width="2" />'
        }

        const yTicks = 4
        const yGrid = Array.from({ length: yTicks + 1 }, (_, i) => {
          const value = yMin + (i / yTicks) * ySpan
          const y = toY(value)
          return {
            line: '<line x1="' + paddingLeft + '" y1="' + y.toFixed(1) + '" x2="' + (width - paddingRight) + '" y2="' + y.toFixed(1) + '" stroke="#243151" stroke-width="1" />',
            label: '<text x="' + (paddingLeft - 6) + '" y="' + (y + 4).toFixed(1) + '" text-anchor="end" fill="#96a4c2" font-size="11">' + value.toFixed(1) + '</text>',
          }
        })

        const isTick = (label) => /^(00|06|12|18):00$/.test(label)
        const lastPoint = points[points.length - 1]
        const xTicks = points
          .filter((point) => isTick(point.label))
          .map((point) => ({ time: point.start, label: point.label.slice(0, 2) }))
          .concat(isTick(lastPoint.endLabel) ? [{ time: lastPoint.end, label: lastPoint.endLabel.slice(0, 2) }] : [])
        const xGrid = xTicks.map((tick) => {
          const x = toX(tick.time)
          return {
            line: '<line x1="' + x.toFixed(1) + '" y1="' + paddingTop + '" x2="' + x.toFixed(1) + '" y2="' + (height - paddingBottom) + '" stroke="#1f2a45" stroke-width="1" />',
            label: '<text x="' + x.toFixed(1) + '" y="' + (height - 6) + '" text-anchor="middle" fill="#96a4c2" font-size="11">' + tick.label + '</text>',
          }
        })

        const crosshairId = svgId + '_crosshair'
        svg.setAttribute('viewBox', '0 0 ' + width + ' ' + height)
        svg.innerHTML =
          '<rect x="0" y="0" width="' + width + '" height="' + height + '" fill="#0e1426" />' +
          yGrid.map((tick) => tick.line).join('') +
          xGrid.map((tick) => tick.line).join('') +
          '<line x1="' + paddingLeft + '" y1="' + (height - paddingBottom) + '" x2="' + (width - paddingRight) + '" y2="' + (height - paddingBottom) + '" stroke="#2b3655" stroke-width="1" />' +
          '<line x1="' + paddingLeft + '" y1="' + paddingTop + '" x2="' + paddingLeft + '" y2="' + (height - paddingBottom) + '" stroke="#2b3655" stroke-width="1" />' +
          yGrid.map((tick) => tick.label).join('') +
          xGrid.map((tick) => tick.label).join('') +
          pathFor(todayData.concat(tomorrowData.slice(0, 1)), '#5dd5ff') +
          pathFor(tomorrowData, '#7b6bff') +
          '<line id="' + crosshairId + '" x1="0" y1="' + paddingTop + '" x2="0" y2="' + (height - paddingBottom) + '" stroke="#7f8fb6" stroke-dasharray="3 3" stroke-width="1" visibility="hidden" />'

        const crosshair = document.getElementById(crosshairId)
        const handleHover = (clientX) => {
          const rect = svg.getBoundingClientRect()
          const x = clientX - rect.left
          const time = domainStart + ((x - paddingLeft) / chartWidth) * domainSpan
          const index = time < domainStart ? -1 : points.findIndex((point) => time < point.end)
          if (index < 0 || index >= points.length) {
            if (tooltip) tooltip.style.display = 'none'
            if (crosshair) crosshair.setAttribute('visibility', 'hidden')
            return
          }

          const point = points[index]
          const xPos = toX(point.start)
          if (crosshair) {
            crosshair.setAttribute('x1', xPos.toFixed(1))
            crosshair.setAttribute('x2', xPos.toFixed(1))
            crosshair.setAttribute('visibility', 'visible')
          }
          if (tooltip) {
            tooltip.textContent = point.label + ' - ' + point.value.toFixed(2) + ' c/kWh'
            tooltip.style.display = 'block'
          }
        }

        svg.onmousemove = (event) => handleHover(event.clientX)
        svg.ontouchmove = (event) => {
          const touch = event.touches[0]
          if (touch) {
            handleHover(touch.clientX)
          }
        }
        svg.onmouseleave = () => {
          if (tooltip) tooltip.style.display = 'none'
          if (crosshair) crosshair.setAttribute('visibility', 'hidden')
        }
      }

      const buildHaYaml = (apiKey, baseUrl) => {
        const auth = 'Bearer ' + apiKey
        return [
          '# Spot Price API - Home Assistant REST Commands',
          '',
          'rest_command:',
          '',
          '  spot_price_now:',
          '    url: "' + baseUrl + '/api/v1/price/now"',
          '    method: GET',
          '    headers:',
          '      Authorization: "' + auth + '"',
          '    content_type: "application/json"',
          '',
          '  spot_price_today:',
          '    url: "' + baseUrl + '/api/v1/price/today"',
          '    method: GET',
          '    headers:',
          '      Authorization: "' + auth + '"',
          '    content_type: "application/json"',
          '',
          '  spot_price_tomorrow:',
          '    url: "' + baseUrl + '/api/v1/price/tomorrow"',
          '    method: GET',
          '    headers:',
          '      Authorization: "' + auth + '"',
          '    content_type: "application/json"',
          '',
          '  spot_price_all:',
          '    url: "' + baseUrl + '/api/v1/price/all"',
          '    method: GET',
          '    headers:',
          '      Authorization: "' + auth + '"',
          '    content_type: "application/json"',
          '',
          '  spot_price_cheapest:',
          '    url: >-',
          '      ' + baseUrl + '/api/v1/price/cheapest?duration={{ duration }}{% if start is defined %}&startTime={{ start | urlencode }}{% endif %}{% if end is defined %}&endTime={{ end | urlencode }}{% endif %}{% if max_price is defined %}&maxPrice={{ max_price }}{% endif %}',
          '    method: GET',
          '    headers:',
          '      Authorization: "' + auth + '"',
          '    content_type: "application/json"',
          '',
          '  spot_price_history:',
          '    url: "' + baseUrl + '/api/v1/price/history?from={{ from }}&to={{ to }}"',
          '    method: GET',
          '    headers:',
          '      Authorization: "' + auth + '"',
          '    content_type: "application/json"',
          '',
          '  spot_price_forecast:',
          '    url: "' + baseUrl + '/api/v1/price/forecast"',
          '    method: GET',
          '    headers:',
          '      Authorization: "' + auth + '"',
          '    content_type: "application/json"',
        ].join('\n')
      }

      const buildHaPackagesInclude = () => [
        'homeassistant:',
        '  packages: !include_dir_named packages',
      ].join('\n')

      const buildHaUsageExample = () => [
        'action: rest_command.spot_price_cheapest',
        'data:',
        '  duration: "210"',
        `  start: '{{  today_at("22:00").isoformat() }}'`,
        `  end:   '{{ (today_at("7:00") + timedelta(days=1)).isoformat() }}'`,
        '  max_price: "20"',
        'response_variable: result',
      ].join('\n')

      const toUtcIsoSeconds = (date) => {
        const iso = date.toISOString()
        return iso.endsWith('.000Z') ? iso.slice(0, -5) + 'Z' : iso
      }

      const buildUtcIsoForTodayHour = (hour) => {
        const now = new Date()
        const timestamp = new Date(
          Date.UTC(
            now.getUTCFullYear(),
            now.getUTCMonth(),
            now.getUTCDate(),
            hour,
            0,
            0,
            0,
          ),
        )
        return toUtcIsoSeconds(timestamp)
      }

      const renderExamples = () => {
        if (!state.apiKey) return
        const base = location.origin
        const apiKeyHeader = '-H "Authorization: Bearer ' + state.apiKey + '"'
        const windowStartUtc = buildUtcIsoForTodayHour(8)
        const windowEndUtc = buildUtcIsoForTodayHour(17)
        const nowMs = Date.now()
        // Display-only UTC calendar dates for a valid <=31-day history span.
        const historyTo = new Date(nowMs).toISOString().slice(0, 10)
        const historyFrom = new Date(nowMs - 7 * 24 * 60 * 60 * 1000).toISOString().slice(0, 10)
        const examples = [
          {
            title: 'Current total price',
            command: 'curl -sS ' + apiKeyHeader + ' ' + base + '/api/v1/price/now',
          },
          {
            title: 'Cheapest 3-hour window',
            command: 'curl -sS ' + apiKeyHeader + ' "' + base + '/api/v1/price/cheapest?duration=180"',
          },
          {
            title: 'Cheapest 1-hour window under 20 c/kWh',
            command:
              'curl -sS ' +
              apiKeyHeader +
              ' "' +
              base +
              '/api/v1/price/cheapest?duration=60&maxPrice=20"',
          },
          {
            title: 'Cheapest 2-hour window today between 08:00-17:00 UTC',
            command:
              'curl -sS ' +
              apiKeyHeader +
              ' "' +
              base +
              '/api/v1/price/cheapest?duration=120&startTime=' +
              windowStartUtc +
              '&endTime=' +
              windowEndUtc +
              '"',
          },
          {
            title: 'Today prices',
            command: 'curl -sS ' + apiKeyHeader + ' ' + base + '/api/v1/price/today',
          },
          {
            title: 'Tomorrow prices',
            command: 'curl -sS ' + apiKeyHeader + ' ' + base + '/api/v1/price/tomorrow',
          },
          {
            title: 'All published prices in one list',
            command: 'curl -sS ' + apiKeyHeader + ' ' + base + '/api/v1/price/all',
          },
          {
            title: 'Price history for a date range',
            command: 'curl -sS ' + apiKeyHeader + ' "' + base + '/api/v1/price/history?from=' + historyFrom + '&to=' + historyTo + '"',
          },
          {
            title: 'Price forecast (estimate, FI only)',
            command: 'curl -sS ' + apiKeyHeader + ' ' + base + '/api/v1/price/forecast',
          },
        ]

        $('apiExamples').innerHTML = examples.map((example, index) => (
          '<div class="example-block">' +
            '<pre><span class="comment"># ' + example.title + '</span>\n' + example.command + '</pre>' +
            '<button class="copy-btn" data-index="' + String(index) + '">Copy</button>' +
          '</div>'
        )).join('')

        const copyButtons = $('apiExamples').querySelectorAll('.copy-btn')
        copyButtons.forEach((button) => {
          button.onclick = async () => {
            const indexText = button.getAttribute('data-index')
            const index = Number(indexText)
            if (!Number.isInteger(index) || index < 0 || index >= examples.length) {
              return
            }
            await copyWithFeedback(button, examples[index].command)
          }
        })

        const copyKeyButton = $('copyKeyBtn')
        if (copyKeyButton) {
          copyKeyButton.onclick = async () => {
            await copyWithFeedback(copyKeyButton, state.apiKey)
          }
        }

        const haPackagesInclude = buildHaPackagesInclude()
        $('haPackagesContent').textContent = haPackagesInclude
        const copyPackagesButton = $('copyHaPackagesBtn')
        if (copyPackagesButton) {
          copyPackagesButton.onclick = async () => {
            await copyWithFeedback(copyPackagesButton, haPackagesInclude)
          }
        }

        const haYaml = buildHaYaml(state.apiKey, base)
        $('haYamlContent').textContent = haYaml
        const copyYamlButton = $('copyHaYamlBtn')
        if (copyYamlButton) {
          copyYamlButton.onclick = async () => {
            await copyWithFeedback(copyYamlButton, haYaml)
          }
        }

        const haUsageExample = buildHaUsageExample()
        $('haUsageContent').textContent = haUsageExample
        const copyUsageButton = $('copyHaUsageBtn')
        if (copyUsageButton) {
          copyUsageButton.onclick = async () => {
            await copyWithFeedback(copyUsageButton, haUsageExample)
          }
        }
      }

      const loadPublicChart = async () => {
        const response = await json('/api/public/spot')
        if (!response.ok) {
          setStatus('publicStatus', 'err', response.data.error || 'Failed')
          return
        }

        const inZone = timeInZone(areaTimezoneMap[response.data.area])
        const startLabel = (price) => inZone(price.deliveryStart)
        const endLabel = (price) => inZone(price.deliveryEnd)
        const today = toChartData(response.data.today || [], 'spotCentsKwh', startLabel, endLabel)
        const tomorrow = toChartData(response.data.tomorrow || [], 'spotCentsKwh', startLabel, endLabel)
        drawChart('publicChart', 'publicTooltip', today, tomorrow)
        setStatus(
          'publicStatus',
          'ok',
          response.data.tomorrowAvailable
            ? 'Today + tomorrow loaded'
            : 'Tomorrow not yet available (published ~12:00 UTC)',
        )
      }

      const loadSession = async () => {
        const response = await json('/api/session')
        if (!response.ok || !response.data.session) {
          state.session = null
          return false
        }

        state.session = response.data.session
        const username = response.data.username || response.data.session.user.name || 'user'
        $('usernameLabel').textContent = username
        return true
      }

      const loadSettings = async () => {
        const response = await json('/api/v1/me/settings')
        if (!response.ok) {
          setStatus('settingsStatus', 'err', response.data.error || 'Failed')
          return
        }

        const settings = response.data
        $('margin').value = settings.marginCentsKwh
        $('dayTransfer').value = settings.transferDayCentsKwh
        $('nightTransfer').value = settings.transferNightCentsKwh
        $('tax').value = settings.taxCentsKwh
        $('vat').value = settings.vatPercent
        $('nightStart').value = settings.nightStartHour
        $('nightEnd').value = settings.nightEndHour
        $('area').value = settings.area
        $('timezone').value = settings.timezone
        setStatus('settingsStatus', 'ok', 'Settings loaded')
      }

      const loadTotalChart = async () => {
        const response = await json('/api/v1/me/chart')
        if (!response.ok) {
          setStatus('totalStatus', 'err', response.data.error || 'Failed')
          return
        }

        const startLabel = (price) => wallClock(price.localStart)
        const endLabel = (price) => wallClock(price.localEnd)
        const today = toChartData(response.data.today || [], 'totalCentsKwh', startLabel, endLabel)
        const tomorrow = toChartData(response.data.tomorrow || [], 'totalCentsKwh', startLabel, endLabel)
        drawChart('totalChart', 'totalTooltip', today, tomorrow)
        setStatus('totalStatus', 'ok', response.data.tomorrowAvailable ? 'Today + tomorrow loaded' : 'Tomorrow not yet available')
      }

      const loadApiKey = async () => {
        const response = await json('/api/keys')
        if (!response.ok) {
          setStatus('apiStatus', 'err', response.data.error || 'Failed')
          return
        }

        state.apiKey = response.data.apiKey
        $('apiKeyDisplay').textContent = state.apiKey
        renderExamples()
      }

      const switchView = (view) => {
        const isDashboard = view === 'dash'
        $('dashView').style.display = isDashboard ? 'grid' : 'none'
        $('apiView').style.display = isDashboard ? 'none' : 'block'
        $('navDash').className = 'btn btn-secondary' + (isDashboard ? ' active' : '')
        $('navApi').className = 'btn btn-secondary' + (isDashboard ? '' : ' active')
        if (!isDashboard) {
          void loadApiKey()
        }
      }

      const showDashboard = async () => {
        $('landing').style.display = 'none'
        $('dashboard').style.display = 'block'
        switchView('dash')
        await Promise.all([loadSettings(), loadTotalChart()])
      }

      $('area').onchange = () => {
        const selectedArea = $('area').value
        const nextTimezone = areaTimezoneMap[selectedArea]
        if (nextTimezone) {
          $('timezone').value = nextTimezone
        }
      }

      $('loginBtn').onclick = () => withLoading('loginBtn', async () => {
        const username = $('username').value.trim().toLowerCase()
        const password = $('password').value
        if (!username || !password) {
          setStatus('authStatus', 'err', 'Username and password required')
          return
        }

        if (password.length < 8 || password.length > 128) {
          setStatus('authStatus', 'err', 'Password must be 8-128 characters.')
          return
        }

        const response = await json('/api/session/login-or-signup', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ username, password }),
        })

        if (!response.ok) {
          const message = response.data.message || response.data.error || 'Login failed'
          setStatus('authStatus', 'err', message)
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
          nightEndHour: Number($('nightEnd').value),
          area: $('area').value,
          timezone: $('timezone').value,
        }

        const response = await json('/api/v1/me/settings', {
          method: 'PUT',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(payload),
        })

        if (!response.ok) {
          setStatus('settingsStatus', 'err', response.data.error || 'Save failed')
          return
        }

        setStatus('settingsStatus', 'ok', 'Settings saved')
        await loadTotalChart()
      })

      $('navDash').onclick = () => switchView('dash')
      $('navApi').onclick = () => switchView('api')

      $('regenBtn').onclick = () => withLoading('regenBtn', async () => {
        const response = await json('/api/keys/regenerate', { method: 'POST' })
        if (!response.ok) {
          setStatus('apiStatus', 'err', response.data.error || 'Failed')
          return
        }

        state.apiKey = response.data.apiKey
        $('apiKeyDisplay').textContent = state.apiKey
        renderExamples()
        setStatus('apiStatus', 'ok', 'New API key generated. Old key is now invalid.')
      })

      ;(async () => {
        await loadPublicChart()
        const hasSession = await loadSession()
        if (hasSession) {
          await showDashboard()
        }
      })()
