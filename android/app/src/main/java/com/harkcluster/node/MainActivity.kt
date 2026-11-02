package com.harkcluster.node

import android.app.Activity
import android.app.ActivityManager
import android.app.AlertDialog
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import android.net.Uri
import android.provider.Settings
import android.view.Gravity
import android.view.View
import android.widget.Button
import android.widget.GridLayout
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import kotlin.concurrent.thread

/** ARGUS node console: status, resources, health checks, log with copy/share. */
class MainActivity : Activity() {
    private val ui = Handler(Looper.getMainLooper())
    private val BG = Color.parseColor("#0B0E14"); private val PANEL = Color.parseColor("#121722")
    private val LINE = Color.parseColor("#232B3A"); private val DIM = Color.parseColor("#7D8798")
    private val TEXT = Color.parseColor("#E6EAF2"); private val CYAN = Color.parseColor("#22D3EE")
    private val GREEN = Color.parseColor("#34D399"); private val AMBER = Color.parseColor("#FBBF24")
    private val RED = Color.parseColor("#F87171")

    private lateinit var pill: TextView; private lateinit var title: TextView; private lateinit var hint: TextView
    private lateinit var toggle: Button; private lateinit var logView: TextView; private lateinit var logScroll: ScrollView
    private lateinit var checks: LinearLayout
    private lateinit var roleTitle: TextView; private lateinit var roleHint: TextView; private lateinit var roleHelper: TextView
    private lateinit var roleSide: TextView; private lateinit var zimStatus: TextView; private lateinit var zimButton: Button
    private lateinit var heatHint: TextView
    private lateinit var wifiToggle: Button; private lateinit var wifiAddr: TextView; private lateinit var wifiInfo: TextView
    private lateinit var wifiWarn: TextView; private lateinit var wifiBattery: Button
    private lateinit var updVersion: TextView; private lateinit var updStatus: TextView; private lateinit var updButton: Button
    private var updDialog: AlertDialog? = null
    private var updAsked: String? = null      // "vn/vc" already asked in this screen session («Позже»)
    private var updForceShow = false
    private val stat = HashMap<String, TextView>()
    private var lastLog = ""

    private fun dp(v: Int) = (v * resources.displayMetrics.density).toInt()
    private fun box(color: Int, stroke: Int = LINE, r: Int = 6) = GradientDrawable().apply {
        setColor(color); cornerRadius = dp(r).toFloat(); setStroke(dp(1), stroke)
    }
    private fun label(t: String, size: Float = 11f, c: Int = DIM, mono: Boolean = true) = TextView(this).apply {
        text = t; textSize = size; setTextColor(c); if (mono) typeface = Typeface.MONOSPACE; letterSpacing = 0.04f
    }
    private fun section(name: String) = label(name.uppercase(), 10f, DIM).apply { setPadding(0, dp(18), 0, dp(6)) }
    private fun btn(t: String, accent: Int, onClick: () -> Unit) = Button(this).apply {
        text = t; isAllCaps = true; typeface = Typeface.MONOSPACE; textSize = 12f; setTextColor(accent)
        background = box(PANEL, accent, 4); setPadding(dp(14), dp(10), dp(14), dp(10)); setOnClickListener { onClick() }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= 33) requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 1)
        window.statusBarColor = BG; window.navigationBarColor = BG

        val root = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(dp(20), dp(24), dp(20), dp(20)) }

        // header
        root.addView(LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL; gravity = Gravity.CENTER_VERTICAL
            addView(label("◆ ARGUS", 18f, CYAN).apply { setTypeface(Typeface.MONOSPACE, Typeface.BOLD) })
            addView(label("  NODE · ${Build.MANUFACTURER} ${Build.MODEL}".uppercase(), 11f, DIM),
                LinearLayout.LayoutParams(0, -2, 1f))
            pill = label("OFF", 11f, DIM).apply { setPadding(dp(10), dp(4), dp(10), dp(4)) }
            addView(pill)
        })

        // status card
        root.addView(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL; background = box(PANEL); setPadding(dp(16), dp(16), dp(16), dp(16))
            title = label("", 26f, TEXT, mono = false).apply { setTypeface(Typeface.DEFAULT, Typeface.BOLD) }
            hint = label("", 13f, DIM, mono = false).apply { setPadding(0, dp(6), 0, dp(14)) }
            toggle = btn("Запустить узел", CYAN) { toggleNode() }
            addView(title); addView(hint); addView(toggle)
        }, LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(16) })

        // 0.3.2 roles: what this device does for Argus right now
        root.addView(section("Роль"))
        root.addView(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL; background = box(PANEL); setPadding(dp(16), dp(14), dp(16), dp(14))
            roleTitle = label("", 18f, TEXT, mono = false).apply { setTypeface(Typeface.DEFAULT, Typeface.BOLD) }
            roleHint = label("", 12f, DIM, mono = false).apply { setPadding(0, dp(4), 0, 0) }
            roleHelper = label("", 12f, TEXT, mono = false).apply { setPadding(0, dp(8), 0, 0) }
            roleSide = label("", 12f, TEXT, mono = false).apply { setPadding(0, dp(6), 0, 0) }
            zimStatus = label("", 11f, DIM, mono = false).apply { setPadding(0, dp(10), 0, dp(6)) }
            zimButton = btn("Выбрать .zim", CYAN) { pickZim() }
            addView(roleTitle); addView(roleHint); addView(roleHelper); addView(roleSide); addView(zimStatus); addView(zimButton)
        })

        // Wi-Fi mode: rpc-server on 0.0.0.0:50052 + UDP beacon so Argus finds the node on the home LAN
        root.addView(section("Wi-Fi режим"))
        root.addView(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL; background = box(PANEL); setPadding(dp(16), dp(14), dp(16), dp(14))
            wifiToggle = btn("Wi-Fi режим: выкл", CYAN) { toggleWifi() }
            wifiAddr = label("—", 30f, TEXT).apply { setTypeface(Typeface.MONOSPACE, Typeface.BOLD); setPadding(0, dp(12), 0, dp(2)); setTextIsSelectable(true) }
            wifiInfo = label("", 11f, DIM, mono = false)
            wifiWarn = label("⚠ Только домашняя сеть: у RPC нет пароля", 12f, AMBER, mono = false).apply { setPadding(0, dp(8), 0, dp(8)) }
            wifiBattery = btn("Отключить оптимизацию батареи", AMBER) { requestBatteryExemption() }
            addView(wifiToggle); addView(wifiAddr); addView(wifiInfo); addView(wifiWarn); addView(wifiBattery)
        })

        // self-update (0.3.1): always asks — «Обновить» / «Позже» / «Пропустить эту версию»
        root.addView(section("Обновление"))
        root.addView(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL; background = box(PANEL); setPadding(dp(16), dp(14), dp(16), dp(14))
            val (vn, vc) = Updater.myVersion(this@MainActivity)
            updVersion = label("Установлена версия $vn ($vc)", 13f, TEXT, mono = false)
            updStatus = label("", 11f, DIM, mono = false).apply { setPadding(0, dp(6), 0, dp(10)) }
            updButton = btn("Проверить обновление", CYAN) { manualUpdateCheck() }
            addView(updVersion); addView(updStatus); addView(updButton)
        })

        // resources grid
        root.addView(section("Ресурсы"))
        val grid = GridLayout(this).apply { columnCount = if (resources.configuration.screenWidthDp > 600) 4 else 2 }
        for (k in listOf("ОЗУ СВОБОДНО", "ОЗУ ВСЕГО", "ЯДРА CPU", "ПОТОКИ", "ВАРИАНТ", "КЭШ", "ПОРТ", "АПТАЙМ", "ПОДКЛ. ПК", "РЕСТАРТЫ",
                "ТЕМПЕРАТУРА", "БАТАРЕЯ", "ТРОТТЛИНГ", "USB-СЕТЬ")) {
            val v = label("—", 18f, TEXT)
            stat[k] = v
            grid.addView(LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL; background = box(PANEL); setPadding(dp(12), dp(10), dp(12), dp(10))
                addView(label(k, 10f, DIM)); addView(v)
            }, GridLayout.LayoutParams(GridLayout.spec(GridLayout.UNDEFINED), GridLayout.spec(GridLayout.UNDEFINED, 1f)).apply {
                width = 0; setMargins(dp(3), dp(3), dp(3), dp(3))
            })
        }
        root.addView(grid)
        heatHint = label("⚠ Если заряжается и греется — ограничь заряд до 85% в настройках батареи", 12f, AMBER, mono = false)
            .apply { setPadding(0, dp(8), 0, 0); visibility = View.GONE }
        root.addView(heatHint)

        // weight cache (rpc-server -c): tensors pushed once over USB are kept on the device
        root.addView(LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL; gravity = Gravity.CENTER_VERTICAL; setPadding(0, dp(8), 0, 0)
            addView(label("Кэш весов хранит слои модели на устройстве — при следующей загрузке их не нужно копировать заново.", 11f, DIM, mono = false),
                LinearLayout.LayoutParams(0, -2, 1f))
            addView(btn("Очистить кэш", AMBER) { clearCache() })
        })

        // health checks
        root.addView(section("Проверки"))
        checks = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        root.addView(checks)

        // log
        root.addView(LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL; gravity = Gravity.CENTER_VERTICAL
            addView(section("Журнал"), LinearLayout.LayoutParams(0, -2, 1f))
            addView(btn("Копировать", CYAN) { copyLog() })
            addView(View(context), LinearLayout.LayoutParams(dp(8), 1))
            addView(btn("Поделиться", CYAN) { shareLog() })
        })
        logView = label("", 11f, Color.parseColor("#B7C0CE")).apply { setTextIsSelectable(true); setPadding(dp(12), dp(10), dp(12), dp(10)) }
        logScroll = ScrollView(this).apply { background = box(Color.parseColor("#080A0F")); addView(logView) }
        root.addView(logScroll, LinearLayout.LayoutParams(-1, dp(260)).apply { topMargin = dp(6) })

        setContentView(ScrollView(this).apply { setBackgroundColor(BG); isFillViewport = true; addView(root) })
        updForceShow = intent?.getBooleanExtra(Updater.EXTRA_SHOW, false) == true
        tick()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent); setIntent(intent)
        if (intent.getBooleanExtra(Updater.EXTRA_SHOW, false)) { updForceShow = true; updAsked = null; maybeAskUpdate() }
    }

    // 0.3.3 «Библиотека»: pick a .zim with the system file picker, copy it into the app (kiwix-serve reads filesDir/zim)
    private val REQ_ZIM = 42
    private fun pickZim() {
        if (Library.importPct in 0..99) return
        runCatching {
            startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("*/*"), REQ_ZIM)
        }.onFailure { Toast.makeText(this, "Нет файлового менеджера: ${it.message}", Toast.LENGTH_LONG).show() }
    }

    @Deprecated("Activity result API is androidx-only; this app has no androidx")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != REQ_ZIM || resultCode != RESULT_OK) return
        val uri = data?.data ?: return
        thread(name = "zim-import") {
            val r = runCatching { Library.import(applicationContext, uri) }
            ui.post { Toast.makeText(this, r.fold({ "Скопировано: $it" }, { "Ошибка: ${it.message}" }), Toast.LENGTH_LONG).show() }
        }
    }

    private fun manualUpdateCheck() {
        if (Updater.busy) return
        updButton.isEnabled = false
        thread(name = "update-check") {
            val a = Updater.manualCheck(this)
            ui.post {
                updButton.isEnabled = true
                if (a != null) { updForceShow = true; updAsked = null; maybeAskUpdate() }
                else Toast.makeText(this, Updater.status.ifEmpty { "Обновлений нет" }, Toast.LENGTH_SHORT).show()
            }
        }
    }

    /** The question. Nothing is downloaded or installed until the user taps «Обновить». */
    private fun maybeAskUpdate() {
        val a = Updater.available ?: return
        val key = "${a.vn}/${a.vc}"
        if (updDialog?.isShowing == true || updAsked == key || isFinishing) return
        if (!updForceShow && !Updater.forced && Updater.skipped(this, a)) return
        updAsked = key; updForceShow = false; Updater.forced = false
        val (myVn, _) = Updater.myVersion(this)
        val notes = a.notes.trim().let { if (it.isEmpty()) "" else "\n\n" + it.take(1200) }
        updDialog = AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
            .setTitle("Доступна версия ${a.vn}. Хотите обновить?")
            .setMessage("Установлена: $myVn\nИсточник: ${a.source}" + (if (a.sha256 != null) "\nКонтрольная сумма SHA-256 будет проверена." else "") +
                "\n\nAndroid попросит подтвердить установку. Узел перезапустится после обновления.$notes")
            .setPositiveButton("Обновить") { _, _ -> startUpdate(a) }
            .setNegativeButton("Позже") { _, _ -> }
            .setNeutralButton("Пропустить эту версию") { _, _ -> Updater.skip(this, a) }
            .show()
    }

    private fun startUpdate(a: Updater.Avail) {
        updButton.isEnabled = false
        thread(name = "update-download") {
            val f = runCatching { Updater.download(this, a) }.onFailure { Updater.status = "Ошибка: ${it.message}"; RpcService.addLog("update: ${it.message}") }.getOrNull()
            ui.post {
                updButton.isEnabled = true
                if (f != null) runCatching { Updater.install(this, f) }.onFailure { Updater.status = "Ошибка установки: ${it.message}" }
                else Toast.makeText(this, Updater.status, Toast.LENGTH_LONG).show()
            }
        }
    }

    private fun toggleNode() {
        val i = Intent(this, RpcService::class.java)
        if (RpcService.running) startService(i.setAction(RpcService.ACTION_STOP)) else startForegroundService(i)
    }

    /** Persisted; if the node is running, the service restarts rpc-server on the new address right away. */
    private fun toggleWifi() {
        val on = !RpcService.wifiPref(this)
        RpcService.setWifiPref(this, on)
        if (RpcService.running) startForegroundService(Intent(this, RpcService::class.java).putExtra("wifi", on))
        if (on && !batteryExempt()) requestBatteryExemption()
        tick(reschedule = false)
    }

    private fun batteryExempt() = getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(packageName)
    private fun requestBatteryExemption() {
        runCatching {
            startActivity(Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).setData(Uri.parse("package:$packageName")))
        }.onFailure { startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)) }
    }

    private fun report(): String {
        val m = memInfo()
        return "ARGUS node ${packageManager.getPackageInfo(packageName, 0).versionName} · ${Build.MANUFACTURER} ${Build.MODEL} · Android ${Build.VERSION.RELEASE}\n" +
            "helper=${Helper.state} model=${Helper.model} pct=${Helper.pct} err=${Helper.error} bench=${Bench.cached(this)}\n" +
            "state=${RpcService.state} mode=${if (RpcService.wifi) "wifi" else "usb"} host=${RpcService.host} ip=${RpcService.wifiIp} port=${RpcService.port} threads=${RpcService.threads} variant=${RpcService.variant} cpu=${RpcService.coreTiers} cache=${fmtBytes(cacheSize)} restarts=${RpcService.restarts} lastExit=${RpcService.lastExit}\n" +
            "ram free=${m.availMem shr 20}MB total=${m.totalMem shr 20}MB\n\n" + RpcService.logText()
    }
    private var cacheSize = -1L; private var cacheTick = 0
    private fun refreshCacheSize() { thread(name = "cache-size") { cacheSize = RpcService.cacheBytes(this) } }
    private fun clearCache() {
        val warn = if (RpcService.clients > 0 && RpcService.state == RpcService.State.LINKED) " (ПК подключён: текущая модель работает, при следующей загрузке веса передадутся заново)" else ""
        thread(name = "cache-clear") {
            RpcService.clearCache(this); cacheSize = RpcService.cacheBytes(this)
            ui.post { Toast.makeText(this, "Кэш очищен$warn", Toast.LENGTH_SHORT).show() }
        }
    }
    private fun fmtBytes(b: Long) = when { b < 0 -> "…"; b < 1L shl 20 -> "${b shr 10} KB"; b < 1L shl 30 -> "${b shr 20} MB"; else -> "%.1f GB".format(b / 1073741824.0) }

    private fun copyLog() {
        getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("argus-log", report()))
        Toast.makeText(this, "Журнал скопирован", Toast.LENGTH_SHORT).show()
    }
    private fun shareLog() = startActivity(Intent.createChooser(
        Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, report()), "Отправить журнал"))

    private fun memInfo() = ActivityManager.MemoryInfo().also { getSystemService(ActivityManager::class.java).getMemoryInfo(it) }

    private fun tick(reschedule: Boolean = true) {
        val s = RpcService.state
        val (word, color, head, sub) = when (s) {
            RpcService.State.OFF -> Quad("OFF", DIM, "Узел выключен", if (RpcService.wifiPref(this)) "Wi-Fi режим включён: нажмите «Запустить узел» — Argus на ПК найдёт его в сети." else "Подключите устройство к ПК с открытым Argus — узел запустится сам. Или нажмите «Запустить».")
            RpcService.State.STARTING -> Quad("STARTING", AMBER, "Запуск…", "Запускается вычислительный сервер.")
            RpcService.State.READY -> Quad("READY", CYAN, "Ждёт ПК", if (RpcService.wifi) "Сервер слушает ${RpcService.wifiIp ?: "0.0.0.0"}:${RpcService.port}. Argus на ПК подключится по Wi-Fi или USB." else "Сервер запущен на порту ${RpcService.port}. Argus на ПК подключится по USB.")
            RpcService.State.LINKED -> Quad("LINKED", GREEN, "ПК подключён", if (RpcService.wifi) "Делится памятью и CPU по Wi-Fi. Держите на зарядке; экран можно выключить." else "Делится памятью и CPU. Не отключайте кабель; экран можно выключить.")
            RpcService.State.RESTARTING -> Quad("RESTART", RED, "Восстановление", "Сервер остановился (${RpcService.lastExit}). Перезапуск автоматически. Если повторяется — нажмите «Поделиться» и отправьте журнал.")
        }
        pill.text = word; pill.setTextColor(color); pill.background = box(PANEL, color, 10)
        title.text = head; title.setTextColor(if (s == RpcService.State.OFF) TEXT else color); hint.text = sub
        toggle.text = if (RpcService.running) "Остановить узел" else "Запустить узел"
        toggle.setTextColor(if (RpcService.running) RED else CYAN); toggle.background = box(PANEL, if (RpcService.running) RED else CYAN, 4)

        val m = memInfo()
        stat["ОЗУ СВОБОДНО"]!!.text = "%.1f GB".format(m.availMem / 1e9)
        stat["ОЗУ ВСЕГО"]!!.text = "%.1f GB".format(m.totalMem / 1e9)
        if (RpcService.bigCores == 0) RpcService.bigCores = RpcService.detectBigCores()
        stat["ЯДРА CPU"]!!.text = "${Runtime.getRuntime().availableProcessors()} (${RpcService.bigCores} больш.)"
        stat["ВАРИАНТ"]!!.text = RpcService.variant
        if (cacheTick++ % 5 == 0) refreshCacheSize()
        stat["КЭШ"]!!.text = fmtBytes(cacheSize)
        stat["ПОТОКИ"]!!.text = if (RpcService.running) RpcService.threads.toString() else "—"
        stat["ПОРТ"]!!.text = if (RpcService.running) RpcService.port.toString() else "—"
        stat["АПТАЙМ"]!!.text = if (RpcService.running) fmtUp((SystemClock.elapsedRealtime() - RpcService.startedAt) / 1000) else "—"
        stat["ПОДКЛ. ПК"]!!.text = RpcService.clients.toString()
        stat["РЕСТАРТЫ"]!!.text = RpcService.restarts.toString()
        stat["РЕСТАРТЫ"]!!.setTextColor(if (RpcService.restarts > 0) AMBER else TEXT)

        val tl = Telemetry.read(this)
        stat["ТЕМПЕРАТУРА"]!!.text = tl.tempC?.let { "%.0f °C".format(it) } ?: "—"
        stat["ТЕМПЕРАТУРА"]!!.setTextColor(when { (tl.tempC ?: 0.0) >= 45 -> RED; (tl.tempC ?: 0.0) >= 40 -> AMBER; else -> TEXT })
        stat["БАТАРЕЯ"]!!.text = if (tl.batteryPct >= 0) "${tl.batteryPct}%" + (if (tl.charging) " ⚡" else "") else "—"
        stat["ТРОТТЛИНГ"]!!.text = if (Build.VERSION.SDK_INT < 29) "—" else if (tl.throttled) "да (${tl.thermalStatus})" else "нет"
        stat["ТРОТТЛИНГ"]!!.setTextColor(if (tl.throttled) RED else TEXT)
        stat["USB-СЕТЬ"]!!.text = tl.usbNetIp ?: "—"
        heatHint.visibility = if (Telemetry.hotWhileCharging(tl)) View.VISIBLE else View.GONE

        renderWifi()
        renderRole()
        renderChecks()
        updStatus.text = buildString {
            if (Updater.progress >= 0) append("Загрузка: ${Updater.progress}%") else append(Updater.status)
            Updater.pcHost?.let { if (isNotEmpty()) append(" · "); append("ПК Argus: $it") }
        }.ifEmpty { "Обновления проверяются с ПК (Wi-Fi режим) или кнопкой — с GitHub." }
        maybeAskUpdate()
        val text = RpcService.logText()
        if (text != lastLog) { lastLog = text; logView.text = text.ifEmpty { "Пока нет вывода." }; logScroll.post { logScroll.fullScroll(View.FOCUS_DOWN) } }
        if (reschedule) ui.postDelayed({ tick() }, 1000)
    }

    private fun renderWifi() {
        val on = RpcService.wifiPref(this)
        wifiToggle.text = if (on) "Wi-Fi режим: вкл" else "Wi-Fi режим: выкл"
        wifiToggle.setTextColor(if (on) GREEN else CYAN); wifiToggle.background = box(PANEL, if (on) GREEN else CYAN, 4)
        val vis = if (on) View.VISIBLE else View.GONE
        wifiAddr.visibility = vis; wifiInfo.visibility = vis; wifiWarn.visibility = vis
        val ip = RpcService.wifiIp ?: RpcService.lanAddress()?.first?.hostAddress
        wifiAddr.text = if (ip != null) "$ip:${RpcService.WIFI_PORT}" else "нет Wi-Fi"
        wifiAddr.setTextColor(if (ip == null) RED else if (RpcService.running && RpcService.wifi) GREEN else TEXT)
        wifiInfo.text = when {
            !RpcService.running -> "Узел не запущен — нажмите «Запустить узел»."
            !RpcService.wifi -> "Применяется…"
            else -> "Слушает 0.0.0.0:${RpcService.port} · маяк UDP :${RpcService.BEACON_PORT} каждые 2 с (отправлено ${RpcService.beacons})"
        }
        wifiBattery.visibility = if (on && !batteryExempt()) View.VISIBLE else View.GONE
    }

    private fun renderRole() {
        val h = Helper.state
        val model = Helper.model?.removeSuffix(".gguf")
        val linked = RpcService.state == RpcService.State.LINKED
        val (t, c, sub) = when {
            h == "idle" && (Voice.active || Library.active) -> Triple(listOfNotNull(if (Voice.active) "Голос" else null, if (Library.active) "Библиотека" else null).joinToString(" + "),
                CYAN, "Работает рядом с основной моделью на малых ядрах." + (if (linked) " Заодно держит часть основной модели." else ""))
            h != "idle" -> Triple("Помощник", CYAN, "Отвечает Argus на небольшие задачи своей маленькой моделью: названия чатов, сводки, параллельные ответы." +
                (if (linked) " Заодно держит часть основной модели." else ""))
            linked -> Triple("Часть модели", GREEN, "Хранит и считает часть слоёв большой модели вместе с ПК.")
            RpcService.running -> Triple("Свободна", TEXT, "Готова к работе — Argus на ПК сам решит, чем её занять.")
            else -> Triple("Свободна", DIM, "Узел выключен. Запустите его, чтобы Argus мог дать устройству роль.")
        }
        roleTitle.text = t; roleTitle.setTextColor(c); roleHint.text = sub
        val bench = Bench.cached(this)
        val benchText = when {
            bench != null -> "Тест скорости: %.1f ГБ/с".format(bench)
            Bench.running -> "Тест скорости: идёт…"
            else -> null
        }
        val (helperText, hc) = when (h) {
            "downloading" -> "Скачиваю модель ${model ?: ""}: ${Helper.pct}%" to AMBER
            "starting" -> (if (Helper.error != null) "Модель ${model ?: ""} перезапускается после сбоя" else "Загружаю модель ${model ?: ""} в память…") to AMBER
            "ready" -> "Модель ${model ?: ""} готова к работе" to GREEN
            "error" -> "Не получилось запустить ${model ?: "модель"}: ${Helper.error ?: "неизвестная ошибка"}" to RED
            else -> "" to DIM
        }
        roleHelper.text = listOfNotNull(helperText.ifEmpty { null }, benchText).joinToString("\n")
        roleHelper.setTextColor(if (helperText.isEmpty()) DIM else hc)
        roleHelper.visibility = if (roleHelper.text.isEmpty()) View.GONE else View.VISIBLE
        // 0.3.3 voice + library lines
        fun side(n: String, st: String, what: String?, err: String?, pct: Int, port: Int) = when (st) {
            "idle" -> null
            "downloading" -> "$n: скачиваю ${what ?: ""} ${pct}%"
            "starting" -> "$n: запускаю ${what ?: ""}…"
            "ready" -> "$n: готов, порт $port (${what ?: ""})"
            else -> "$n: ошибка — ${err ?: "неизвестно"}"
        }
        val lines = listOfNotNull(side("Голос", Voice.state, Voice.model, Voice.error, Voice.pct, Voice.PORT),
            side("Библиотека", Library.state, Library.zim, Library.error, 0, Library.PORT))
        roleSide.text = lines.joinToString("\n")
        roleSide.visibility = if (lines.isEmpty()) View.GONE else View.VISIBLE
        roleSide.setTextColor(if (Voice.state == "error" || Library.state == "error") RED else TEXT)
        val zs = Library.zims(this)
        zimStatus.text = when {
            Library.importPct in 0..99 -> "Копирую .zim: ${Library.importPct}%"
            Library.importMsg != null -> Library.importMsg!! + if (zs.isNotEmpty()) "\nНа телефоне: ${zs.joinToString { it.name }}" else ""
            zs.isNotEmpty() -> "Офлайн-библиотека на телефоне: ${zs.joinToString { it.name }}"
            else -> "Библиотека: выберите .zim (офлайн-Википедия Kiwix) — ПК сможет открыть её через этот телефон."
        }
        zimButton.isEnabled = Library.importPct !in 0..99
    }

    private fun renderChecks() {
        val adb = Settings.Global.getInt(contentResolver, Settings.Global.ADB_ENABLED, 0) == 1
        val battery = getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(packageName)
        val notif = Build.VERSION.SDK_INT < 33 || checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) == 0
        checks.removeAllViews()
        checks.addView(checkRow("Отладка по USB", adb, "Настройки › Для разработчиков › Отладка по USB") {
            startActivity(Intent(Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS)) })
        checks.addView(checkRow("Батарея: без ограничений", battery, "Узел не засыпает при выключенном экране") { requestBatteryExemption() })
        checks.addView(checkRow("Уведомления", notif, "Нужны, чтобы показывать, что узел работает") {
            startActivity(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, packageName)) })
    }

    private fun checkRow(name: String, ok: Boolean, help: String, fix: () -> Unit) = LinearLayout(this).apply {
        orientation = LinearLayout.HORIZONTAL; gravity = Gravity.CENTER_VERTICAL
        background = box(PANEL, if (ok) LINE else AMBER); setPadding(dp(12), dp(10), dp(12), dp(10))
        layoutParams = LinearLayout.LayoutParams(-1, -2).apply { bottomMargin = dp(6) }
        addView(label(if (ok) "●" else "▲", 14f, if (ok) GREEN else AMBER).apply { setPadding(0, 0, dp(10), 0) })
        addView(LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            addView(label(name, 13f, TEXT, mono = false)); if (!ok) addView(label(help, 11f, DIM, mono = false))
        }, LinearLayout.LayoutParams(0, -2, 1f))
        if (!ok) addView(btn("Исправить", AMBER, fix))
    }

    private fun fmtUp(s: Long) = if (s < 3600) "%d:%02d".format(s / 60, s % 60) else "%dh%02dm".format(s / 3600, s / 60 % 60)
    private data class Quad(val a: String, val b: Int, val c: String, val d: String)

    override fun onDestroy() { ui.removeCallbacksAndMessages(null); super.onDestroy() }
}
