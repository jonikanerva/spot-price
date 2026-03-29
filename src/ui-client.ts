export const renderHomePageClientScript = (areaTimezoneMap: string): string => `
      const $ = (id) => document.getElementById(id)

      const state = { session: null, apiKey: '' }
      const areaTimezoneMap = ${areaTimezoneMap}

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

      const toChartData = (prices, valueKey) =>
        prices.map((price) => ({
          value: Number(price[valueKey]),
          label: new Date(price.deliveryStart).toLocaleTimeString('fi-FI', {
            hour: '2-digit',
            minute: '2-digit',
            hour12: false,
          }),
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

        const toX = (index) =>
          paddingLeft +
          (points.length <= 1 ? 0 : (index / (points.length - 1)) * chartWidth)
        const toY = (value) =>
          paddingTop + (1 - (value - yMin) / ySpan) * chartHeight

        const pathFor = (series, offset, color) => {
          if (series.length === 0) return ''
          const d = series
            .map((point, i) => {
              const cmd = i === 0 ? 'M' : 'L'
              return cmd + toX(offset + i).toFixed(1) + ' ' + toY(point.value).toFixed(1)
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

        const xTicks = ['00', '06', '12', '18', '00']
        const xGrid = [0, 0.25, 0.5, 0.75, 1].map((factor, i) => {
          const x = paddingLeft + factor * chartWidth
          return {
            line: '<line x1="' + x.toFixed(1) + '" y1="' + paddingTop + '" x2="' + x.toFixed(1) + '" y2="' + (height - paddingBottom) + '" stroke="#1f2a45" stroke-width="1" />',
            label: '<text x="' + x.toFixed(1) + '" y="' + (height - 6) + '" text-anchor="middle" fill="#96a4c2" font-size="11">' + xTicks[i] + '</text>',
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
          pathFor(todayData, 0, '#5dd5ff') +
          pathFor(tomorrowData, todayData.length, '#7b6bff') +
          '<line id="' + crosshairId + '" x1="0" y1="' + paddingTop + '" x2="0" y2="' + (height - paddingBottom) + '" stroke="#7f8fb6" stroke-dasharray="3 3" stroke-width="1" visibility="hidden" />'

        const crosshair = document.getElementById(crosshairId)
        const handleHover = (clientX) => {
          const rect = svg.getBoundingClientRect()
          const x = clientX - rect.left
          const index = Math.round(((x - paddingLeft) / chartWidth) * (points.length - 1))
          if (index < 0 || index >= points.length) {
            if (tooltip) tooltip.style.display = 'none'
            if (crosshair) crosshair.setAttribute('visibility', 'hidden')
            return
          }

          const point = points[index]
          const xPos = toX(index)
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
          '  spot_price_cheapest:',
          '    url: >-',
          '      ' + baseUrl + '/api/v1/price/cheapest?duration={{ duration }}{% if start is defined %}&startTime={{ start | urlencode }}{% endif %}{% if end is defined %}&endTime={{ end | urlencode }}{% endif %}{% if max_price is defined %}&maxPrice={{ max_price }}{% endif %}',
          '    method: GET',
          '    headers:',
          '      Authorization: "' + auth + '"',
          '    content_type: "application/json"',
        ].join('\\n')
      }

      const buildHaPackagesInclude = () => [
        'homeassistant:',
        '  packages: !include_dir_named packages',
      ].join('\\n')

      const buildHaUsageExample = () => [
        'action: rest_command.spot_price_cheapest',
        'data:',
        '  duration: "210"',
        \`  start: '{{  today_at("22:00").isoformat() }}'\`,
        \`  end:   '{{ (today_at("7:00") + timedelta(days=1)).isoformat() }}'\`,
        '  max_price: "20"',
        'response_variable: result',
      ].join('\\n')

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
        ]

        $('apiExamples').innerHTML = examples.map((example, index) => (
          '<div class="example-block">' +
            '<pre><span class="comment"># ' + example.title + '</span>\\n' + example.command + '</pre>' +
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

        const today = toChartData(response.data.today || [], 'spotCentsKwh')
        const tomorrow = toChartData(response.data.tomorrow || [], 'spotCentsKwh')
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

        const today = toChartData(response.data.today || [], 'totalCentsKwh')
        const tomorrow = toChartData(response.data.tomorrow || [], 'totalCentsKwh')
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
`;
