package com.harkcluster.node

import android.app.Activity
import android.app.ActivityManager
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
            toggle = btn("Start node", CYAN) { toggleNode() }
            addView(title); addView(hint); addView(toggle)
        }, LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(16) })

        // resources grid
        root.addView(section("Resources"))
        val grid = GridLayout(this).apply { columnCount = if (resources.configuration.screenWidthDp > 600) 4 else 2 }
        for (k in listOf("RAM FREE", "RAM TOTAL", "CPU CORES", "THREADS", "VARIANT", "CACHE", "PORT", "UPTIME", "PC LINKS", "RESTARTS")) {
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

        // weight cache (rpc-server -c): tensors pushed once over USB are kept on the device
        root.addView(LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL; gravity = Gravity.CENTER_VERTICAL; setPadding(0, dp(8), 0, 0)
            addView(label("Weight cache keeps model layers on the device so the next load skips the USB copy.", 11f, DIM, mono = false),
                LinearLayout.LayoutParams(0, -2, 1f))
            addView(btn("Clear cache", AMBER) { clearCache() })
        })

        // health checks
        root.addView(section("Checks"))
        checks = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        root.addView(checks)

        // log
        root.addView(LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL; gravity = Gravity.CENTER_VERTICAL
            addView(section("Log"), LinearLayout.LayoutParams(0, -2, 1f))
            addView(btn("Copy", CYAN) { copyLog() })
            addView(View(context), LinearLayout.LayoutParams(dp(8), 1))
            addView(btn("Share", CYAN) { shareLog() })
        })
        logView = label("", 11f, Color.parseColor("#B7C0CE")).apply { setTextIsSelectable(true); setPadding(dp(12), dp(10), dp(12), dp(10)) }
        logScroll = ScrollView(this).apply { background = box(Color.parseColor("#080A0F")); addView(logView) }
        root.addView(logScroll, LinearLayout.LayoutParams(-1, dp(260)).apply { topMargin = dp(6) })

        setContentView(ScrollView(this).apply { setBackgroundColor(BG); isFillViewport = true; addView(root) })
        tick()
    }

    private fun toggleNode() {
        val i = Intent(this, RpcService::class.java)
        if (RpcService.running) startService(i.setAction(RpcService.ACTION_STOP)) else startForegroundService(i)
    }

    private fun report(): String {
        val m = memInfo()
        return "ARGUS node ${packageManager.getPackageInfo(packageName, 0).versionName} · ${Build.MANUFACTURER} ${Build.MODEL} · Android ${Build.VERSION.RELEASE}\n" +
            "state=${RpcService.state} port=${RpcService.port} threads=${RpcService.threads} variant=${RpcService.variant} cpu=${RpcService.coreTiers} cache=${fmtBytes(cacheSize)} restarts=${RpcService.restarts} lastExit=${RpcService.lastExit}\n" +
            "ram free=${m.availMem shr 20}MB total=${m.totalMem shr 20}MB\n\n" + RpcService.logText()
    }
    private var cacheSize = -1L; private var cacheTick = 0
    private fun refreshCacheSize() { thread(name = "cache-size") { cacheSize = RpcService.cacheBytes(this) } }
    private fun clearCache() {
        val warn = if (RpcService.clients > 0 && RpcService.state == RpcService.State.LINKED) " (PC is linked; the current model keeps working, next load re-sends weights)" else ""
        thread(name = "cache-clear") {
            RpcService.clearCache(this); cacheSize = RpcService.cacheBytes(this)
            ui.post { Toast.makeText(this, "Cache cleared$warn", Toast.LENGTH_SHORT).show() }
        }
    }
    private fun fmtBytes(b: Long) = when { b < 0 -> "…"; b < 1L shl 20 -> "${b shr 10} KB"; b < 1L shl 30 -> "${b shr 20} MB"; else -> "%.1f GB".format(b / 1073741824.0) }

    private fun copyLog() {
        getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("argus-log", report()))
        Toast.makeText(this, "Log copied", Toast.LENGTH_SHORT).show()
    }
    private fun shareLog() = startActivity(Intent.createChooser(
        Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, report()), "Send log"))

    private fun memInfo() = ActivityManager.MemoryInfo().also { getSystemService(ActivityManager::class.java).getMemoryInfo(it) }

    private fun tick() {
        val s = RpcService.state
        val (word, color, head, sub) = when (s) {
            RpcService.State.OFF -> Quad("OFF", DIM, "Node is off", "Plug the tablet into the PC with Argus open — it starts automatically. Or press Start.")
            RpcService.State.STARTING -> Quad("STARTING", AMBER, "Starting…", "Launching the compute server.")
            RpcService.State.READY -> Quad("READY", CYAN, "Waiting for PC", "Server is up on port ${RpcService.port}. Argus on the PC will connect over USB.")
            RpcService.State.LINKED -> Quad("LINKED", GREEN, "Connected to PC", "Sharing memory and CPU. Keep the cable in; the screen can be off.")
            RpcService.State.RESTARTING -> Quad("RESTART", RED, "Recovering", "Server stopped (${RpcService.lastExit}). Restarting automatically. If this repeats, tap Share and send the log.")
        }
        pill.text = word; pill.setTextColor(color); pill.background = box(PANEL, color, 10)
        title.text = head; title.setTextColor(if (s == RpcService.State.OFF) TEXT else color); hint.text = sub
        toggle.text = if (RpcService.running) "Stop node" else "Start node"
        toggle.setTextColor(if (RpcService.running) RED else CYAN); toggle.background = box(PANEL, if (RpcService.running) RED else CYAN, 4)

        val m = memInfo()
        stat["RAM FREE"]!!.text = "%.1f GB".format(m.availMem / 1e9)
        stat["RAM TOTAL"]!!.text = "%.1f GB".format(m.totalMem / 1e9)
        if (RpcService.bigCores == 0) RpcService.bigCores = RpcService.detectBigCores()
        stat["CPU CORES"]!!.text = "${Runtime.getRuntime().availableProcessors()} (${RpcService.bigCores} big)"
        stat["VARIANT"]!!.text = RpcService.variant
        if (cacheTick++ % 5 == 0) refreshCacheSize()
        stat["CACHE"]!!.text = fmtBytes(cacheSize)
        stat["THREADS"]!!.text = if (RpcService.running) RpcService.threads.toString() else "—"
        stat["PORT"]!!.text = if (RpcService.running) RpcService.port.toString() else "—"
        stat["UPTIME"]!!.text = if (RpcService.running) fmtUp((SystemClock.elapsedRealtime() - RpcService.startedAt) / 1000) else "—"
        stat["PC LINKS"]!!.text = RpcService.clients.toString()
        stat["RESTARTS"]!!.text = RpcService.restarts.toString()
        stat["RESTARTS"]!!.setTextColor(if (RpcService.restarts > 0) AMBER else TEXT)

        renderChecks()
        val text = RpcService.logText()
        if (text != lastLog) { lastLog = text; logView.text = text.ifEmpty { "No output yet." }; logScroll.post { logScroll.fullScroll(View.FOCUS_DOWN) } }
        ui.postDelayed(::tick, 1000)
    }

    private fun renderChecks() {
        val adb = Settings.Global.getInt(contentResolver, Settings.Global.ADB_ENABLED, 0) == 1
        val battery = getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(packageName)
        val notif = Build.VERSION.SDK_INT < 33 || checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) == 0
        checks.removeAllViews()
        checks.addView(checkRow("USB debugging", adb, "Settings › Developer options › USB debugging") {
            startActivity(Intent(Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS)) })
        checks.addView(checkRow("Battery: unrestricted", battery, "Keeps the node alive with the screen off") {
            startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)) })
        checks.addView(checkRow("Notifications", notif, "Needed to show the node is running") {
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
        if (!ok) addView(btn("Fix", AMBER, fix))
    }

    private fun fmtUp(s: Long) = if (s < 3600) "%d:%02d".format(s / 60, s % 60) else "%dh%02dm".format(s / 3600, s / 60 % 60)
    private data class Quad(val a: String, val b: Int, val c: String, val d: String)

    override fun onDestroy() { ui.removeCallbacksAndMessages(null); super.onDestroy() }
}
