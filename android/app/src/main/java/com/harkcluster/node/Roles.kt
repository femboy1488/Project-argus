package com.harkcluster.node

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.Uri
import android.os.BatteryManager
import android.os.Build
import android.os.PowerManager
import android.os.StatFs
import android.os.SystemClock
import android.provider.OpenableColumns
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.net.HttpURLConnection
import java.net.Inet4Address
import java.net.NetworkInterface
import java.net.URL

/**
 * 0.3.3 «Протокол ролей ноды» (docs/ROLES.md). Roles next to rpc-server:
 *   helper  — llama-server :50062 (Helper.kt)
 *   voice   — whisper-server :50061 (Voice below), model downloaded from the PC (:50055/helper/<file>)
 *   library — kiwix-serve :50063 (Library below), .zim picked on the phone (SAF) into filesDir/zim
 * Every assignment is persisted in prefs "node"/"role_<name>" and relaunched when the service is recreated;
 * every stop (role_stop, role=idle, legacy helper_stop, beacon `"<role>":false`) also forgets it.
 */
object Roles {
    const val TAG = "ArgusRoles"

    /** adb extras (RpcService.onStartCommand). Returns a short description for the log, or null if no role extra. */
    fun handleExtras(ctx: Context, intent: Intent?, wifi: Boolean) {
        val app = ctx.applicationContext
        if (intent != null) {
            // stop first, so `--es role_stop all --es role helper` = "only the helper"
            intent.getStringExtra("role_stop")?.let { stopList(app, it, "PC: role_stop=$it") }
            if (intent.getBooleanExtra("helper_stop", false)) Helper.stop("PC: helper_stop", forget = true, ctx = app)
            val roles = intent.getStringExtra("role")?.split(',')?.map { it.trim().lowercase() }?.filter { it.isNotEmpty() }.orEmpty()
            for (r in roles) when (r) {
                "rpc" -> {}                                         // rpc-server always runs: nothing to do
                "idle" -> stopList(app, "all", "PC: role=idle")
                "helper" -> startHelper(app, intent, wifi, explicit = true)
                "voice" -> startVoice(app, intent, wifi)
                "library" -> startLibrary(app, intent.getStringExtra("zim"), wifi)
                else -> RpcService.addLog("roles: unknown role '$r' (rpc|helper|voice|library|idle)")
            }
            // legacy 0.3.2 form: helper_model + helper_url without --es role
            if ("helper" !in roles && !intent.hasExtra("role_stop") && intent.hasExtra("helper_model")) startHelper(app, intent, wifi, explicit = false)
            if ("voice" !in roles && intent.hasExtra("voice_model") && intent.hasExtra("voice_url")) startVoice(app, intent, wifi)
            if ("library" !in roles && intent.hasExtra("zim")) startLibrary(app, intent.getStringExtra("zim"), wifi)
        }
        // address follows the mode; persisted roles come back after a service restart
        Helper.onModeChanged(app, wifi); Voice.onModeChanged(app, wifi); Library.onModeChanged(app, wifi)
        Helper.restore(app, wifi); Voice.restore(app, wifi); Library.restore(app, wifi)
    }

    private fun stopList(ctx: Context, list: String, why: String) {
        val names = list.split(',').map { it.trim().lowercase() }
        val all = "all" in names
        if (all || "helper" in names) Helper.stop(why, forget = true, ctx = ctx)
        if (all || "voice" in names) Voice.stop(why, forget = true, ctx = ctx)
        if (all || "library" in names) Library.stop(why, forget = true, ctx = ctx)
    }

    private fun intOrLong(i: Intent, k: String): Long = i.getLongExtra(k, i.getIntExtra(k, 0).toLong())

    private fun startHelper(ctx: Context, i: Intent, wifi: Boolean, explicit: Boolean) {
        val file = i.getStringExtra("helper_model")?.trim().orEmpty()
        val url = i.getStringExtra("helper_url")?.trim().orEmpty()
        val threads = i.getIntExtra("helper_threads", 0)
        if (file.isNotEmpty() && url.isNotEmpty()) Helper.start(ctx, file, url, intOrLong(i, "helper_size"), wifi, fromLan = false, wantThreads = threads)
        else if (explicit) { if (!Helper.active) Helper.restore(ctx, wifi); if (!Helper.active) RpcService.addLog("roles: role=helper needs --es helper_model and --es helper_url") }
    }

    private fun startVoice(ctx: Context, i: Intent, wifi: Boolean) {
        val file = i.getStringExtra("voice_model")?.trim().orEmpty()
        val url = i.getStringExtra("voice_url")?.trim().orEmpty()
        if (file.isNotEmpty() && url.isNotEmpty()) Voice.start(ctx, file, url, intOrLong(i, "voice_size"), wifi, i.getIntExtra("voice_threads", 0))
        else { if (!Voice.active) Voice.restore(ctx, wifi); if (!Voice.active) RpcService.addLog("roles: role=voice needs --es voice_model and --es voice_url") }
    }

    private fun startLibrary(ctx: Context, zim: String?, wifi: Boolean) {
        val z = zim?.trim()?.takeIf { it.isNotEmpty() } ?: RolePrefs.load(ctx, Library.PREF)?.optString("zim")?.takeIf { it.isNotEmpty() }
            ?: Library.newestZim(ctx)?.name
        if (z == null) { Library.fail("нет .zim — выберите файл на телефоне («Выбрать .zim»)"); return }
        Library.start(ctx, z, wifi)
    }

    /** PC's reply to the beacon: "voice":{file,path,size,threads}|false, "library":{zim}|false ("helper" is Helper's). */
    fun onHostReply(ctx: Context, pcIp: String, msg: JSONObject) {
        Helper.onHostReply(ctx, pcIp, msg)
        val port = msg.optInt("update_port", 50055)
        when (val v = if (msg.has("voice") && !msg.isNull("voice")) msg.opt("voice") else null) {
            is Boolean -> if (!v) Voice.stop("PC (Wi-Fi): role removed", forget = true, ctx = ctx)
            is JSONObject -> v.optString("file").takeIf { it.isNotEmpty() }?.let { f ->
                Voice.start(ctx, f, "http://$pcIp:$port${v.optString("path", "/helper/$f")}", v.optLong("size", 0L), true, v.optInt("threads", 0))
            }
        }
        when (val l = if (msg.has("library") && !msg.isNull("library")) msg.opt("library") else null) {
            is Boolean -> if (!l) Library.stop("PC (Wi-Fi): role removed", forget = true, ctx = ctx)
            is JSONObject -> startLibrary(ctx, l.optString("zim").takeIf { it.isNotEmpty() }, true)
        }
    }

    /** roles map for the beacon / `argus-status`: {helper,voice,library:{state,port,model|zim,...}} */
    fun json(): JSONObject = JSONObject()
        .put("helper", Helper.beaconJson())
        .put("voice", Voice.json())
        .put("library", Library.json())
}

/** Persisted role assignments: prefs "node", key role_<name> = JSON. */
object RolePrefs {
    private fun p(ctx: Context) = ctx.getSharedPreferences(RpcService.PREFS, Context.MODE_PRIVATE)
    fun save(ctx: Context, key: String, j: JSONObject) = p(ctx).edit().putString(key, j.toString()).apply()
    fun load(ctx: Context, key: String): JSONObject? = p(ctx).getString(key, null)?.let { runCatching { JSONObject(it) }.getOrNull() }
    fun clear(ctx: Context, key: String) = p(ctx).edit().remove(key).apply()
}

/** CPU topology for side roles: rpc-server keeps the big cores, side roles go to the little ones. */
object Cpu {
    private fun maxFreqs(): List<Pair<Int, Long>> = (0 until 64).mapNotNull { i ->
        runCatching { i to File("/sys/devices/system/cpu/cpu$i/cpufreq/cpuinfo_max_freq").readText().trim().toLong() }.getOrNull()
    }

    /** Indices of the lowest-frequency tier; empty when all cores are alike or unknown. */
    fun littleCores(): List<Int> {
        val f = maxFreqs(); if (f.isEmpty()) return emptyList()
        val lowest = f.minOf { it.second }
        if (f.all { it.second == lowest }) return emptyList()
        return f.filter { it.second == lowest }.map { it.first }
    }

    /** Default -t for helper/voice: little cores count, at least 2. */
    fun defaultSideThreads(): Int = littleCores().size.coerceAtLeast(2)

    @Volatile private var tasksetOk: Boolean? = null
    private const val TOYBOX = "/system/bin/toybox"

    /** `toybox taskset` works for this uid (probed once with `toybox true`). */
    fun tasksetWorks(): Boolean {
        tasksetOk?.let { return it }
        val ok = File(TOYBOX).exists() && runCatching {
            val p = ProcessBuilder(TOYBOX, "taskset", "1", TOYBOX, "true").redirectErrorStream(true).start()
            p.inputStream.readBytes(); p.waitFor() == 0
        }.getOrDefault(false)
        tasksetOk = ok; return ok
    }

    /** Prefix [cmd] with `toybox taskset <little mask>` when possible and the threads fit on the little cores. */
    fun pinLittle(cmd: List<String>, threads: Int): List<String> {
        val little = littleCores()
        if (little.isEmpty() || threads > little.size || !tasksetWorks()) return cmd
        val mask = little.fold(0L) { m, c -> m or (1L shl c) }
        return listOf(TOYBOX, "taskset", java.lang.Long.toHexString(mask)) + cmd
    }
}

/** 0.3.3 telemetry for the PC (beacon JSON + `argus-status` dumpsys line) and the UI. */
object Telemetry {
    data class Snap(
        val tempC: Double?, val batteryTempC: Double?, val cpuTempC: Double?, val thermalStatus: Int,
        val throttled: Boolean, val batteryPct: Int, val charging: Boolean, val usbNetIp: String?,
    )
    @Volatile private var last: Snap? = null
    @Volatile private var lastAt = 0L

    fun read(ctx: Context): Snap {
        val now = SystemClock.elapsedRealtime()
        last?.let { if (now - lastAt < 3000) return it }
        val bi = runCatching { ctx.applicationContext.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED)) }.getOrNull()
        val bt = bi?.getIntExtra(BatteryManager.EXTRA_TEMPERATURE, Int.MIN_VALUE)?.takeIf { it != Int.MIN_VALUE }?.let { it / 10.0 }
        val level = bi?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: -1
        val scale = bi?.getIntExtra(BatteryManager.EXTRA_SCALE, 100) ?: 100
        val pct = if (level >= 0 && scale > 0) level * 100 / scale else -1
        val plugged = bi?.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) ?: 0
        val st = bi?.getIntExtra(BatteryManager.EXTRA_STATUS, -1) ?: -1
        val charging = plugged != 0 && st != BatteryManager.BATTERY_STATUS_DISCHARGING
        val cpu = zoneMax()
        val ts = if (Build.VERSION.SDK_INT >= 29) runCatching { ctx.getSystemService(PowerManager::class.java).currentThermalStatus }.getOrDefault(0) else 0
        val throttled = Build.VERSION.SDK_INT >= 29 && ts >= PowerManager.THERMAL_STATUS_MODERATE
        val t = listOfNotNull(bt, cpu).maxOrNull()
        return Snap(t, bt, cpu, ts, throttled, pct, charging, usbNetIp()).also { last = it; lastAt = now }
    }

    /** Max of /sys/class/thermal/thermal_zone* /temp (often SELinux-denied for apps → null). */
    private fun zoneMax(): Double? = runCatching {
        File("/sys/class/thermal").listFiles { f -> f.name.startsWith("thermal_zone") }?.mapNotNull { z ->
            val raw = runCatching { File(z, "temp").readText().trim().toDouble() }.getOrNull() ?: return@mapNotNull null
            val c = if (raw > 1000 || raw < -1000) raw / 1000.0 else raw
            c.takeIf { it > 0 && it < 125 }
        }?.maxOrNull()
    }.getOrNull()

    /** IPv4 on a USB tethering / RNDIS / NCM interface (USB networking mode). */
    fun usbNetIp(): String? = runCatching {
        NetworkInterface.getNetworkInterfaces().toList()
            .filter { it.isUp && (it.name.startsWith("rndis") || it.name.startsWith("ncm") || it.name.startsWith("usb")) }
            .flatMap { it.inetAddresses.toList() }.firstOrNull { it is Inet4Address }?.hostAddress
    }.getOrNull()

    /** Fields merged into the beacon JSON and the `argus-status` line. */
    fun put(ctx: Context, j: JSONObject): JSONObject {
        val s = read(ctx)
        s.tempC?.let { j.put("temp_c", Math.round(it * 10) / 10.0) }
        s.batteryTempC?.let { j.put("battery_temp_c", Math.round(it * 10) / 10.0) }
        s.cpuTempC?.let { j.put("cpu_temp_c", Math.round(it * 10) / 10.0) }
        j.put("thermal_status", s.thermalStatus).put("throttled", s.throttled)
        if (s.batteryPct >= 0) j.put("battery_pct", s.batteryPct)
        j.put("charging", s.charging)
        s.usbNetIp?.let { j.put("usb_net_ip", it) }
        return j
    }

    /** UI hint: charging + hot (≥40 °C) or throttled. */
    fun hotWhileCharging(s: Snap) = s.charging && ((s.tempC ?: 0.0) >= 40.0 || s.throttled)
}

/**
 * A side server process (whisper-server / kiwix-serve) supervised like Helper: restart with backoff,
 * health poll on GET / until it answers, gen counter so stale threads quit.
 */
abstract class SideServer(val name: String, val port: Int, private val lib: String, val pref: String) {
    /** idle | downloading | copying | starting | ready | error */
    @Volatile var state = "idle"; protected set
    @Volatile var pct = 0; protected set
    @Volatile var error: String? = null; protected set
    @Volatile var host = "127.0.0.1"; protected set
    @Volatile var restarts = 0; protected set
    @Volatile var threads = 0; protected set
    protected val lock = Any()
    @Volatile protected var gen = 0
    @Volatile private var proc: Process? = null
    private var worker: Thread? = null
    val active get() = state != "idle"

    protected fun log(s: String) { Log.i("Argus-$name", s); RpcService.addLog("$name: $s") }
    protected fun alive(g: Int) = g == gen && !Thread.currentThread().isInterrupted
    protected abstract fun hostArg(wifi: Boolean): String
    protected abstract fun prepare(ctx: Context, g: Int): File?            // model / zim to serve, null = cancelled
    protected abstract fun args(bin: File, target: File): List<String>
    protected open fun persistOnStop() {}

    fun fail(msg: String) { error = msg; state = "error"; log("error: $msg") }

    protected fun launch(ctx: Context, wifi: Boolean) {
        val app = ctx.applicationContext
        host = hostArg(wifi); error = null; pct = 0; restarts = 0; state = "starting"
        val g = ++gen
        worker = Thread({
            val f = try { prepare(app, g) } catch (_: InterruptedException) { return@Thread }
                catch (e: Exception) { synchronized(lock) { if (g == gen) fail(e.message ?: e.toString()) }; return@Thread }
            if (f != null && alive(g)) supervise(app, f, g)
        }, "argus-$name").also { it.isDaemon = true; it.start() }
    }

    fun stop(why: String, forget: Boolean = false, ctx: Context? = null) = synchronized(lock) {
        if (forget && ctx != null) RolePrefs.clear(ctx, pref)
        if (active || proc != null) { stopLocked(); log("stopped ($why)") }
    }

    protected fun stopLocked() {
        gen++; proc?.destroy(); proc = null; worker?.interrupt(); worker = null
        state = "idle"; pct = 0; error = null; resetTarget()
    }
    protected abstract fun resetTarget()

    private fun killOrphans() {
        val me = android.os.Process.myPid(); val own = proc
        File("/proc").listFiles { f -> f.name.all(Char::isDigit) }?.forEach { d ->
            val pid = d.name.toInt(); if (pid == me) return@forEach
            val cmd = runCatching { File(d, "cmdline").readText() }.getOrNull() ?: return@forEach
            if (cmd.contains(lib) && (own == null || !own.isAlive)) { android.os.Process.killProcess(pid); log("killed stale pid $pid") }
        }
    }

    private fun supervise(ctx: Context, target: File, g: Int) {
        val bin = File(ctx.applicationInfo.nativeLibraryDir, lib)
        if (!bin.exists()) { synchronized(lock) { if (g == gen) fail("в этой сборке нет $lib") }; return }
        var backoff = 1000L; var quick = 0
        killOrphans()
        try {
            while (alive(g)) {
                val cmd = args(bin, target)
                log("exec: " + cmd.joinToString(" ") { it.substringAfterLast('/') })
                state = "starting"
                val started = SystemClock.elapsedRealtime()
                val p = try {
                    ProcessBuilder(cmd).redirectErrorStream(true).directory(ctx.filesDir)
                        .apply { environment()["HOME"] = ctx.filesDir.absolutePath; environment()["TMPDIR"] = ctx.cacheDir.absolutePath }
                        .start()
                } catch (e: Exception) { synchronized(lock) { if (g == gen) fail("не запускается $lib: ${e.message}") }; return }
                synchronized(lock) { if (g != gen) { p.destroy(); return }; proc = p }
                val poll = Thread({ pollHealth(p, g) }, "argus-$name-health").apply { isDaemon = true; start() }
                var lastLine = ""
                p.inputStream.bufferedReader().forEachLine { line ->
                    if (line.isBlank()) return@forEachLine
                    lastLine = line
                    if (line.contains("error", true) || line.contains("listen", true) || line.contains("fail", true)) log(line.take(300))
                    else Log.d("Argus-$name", line)
                }
                val code = runCatching { p.waitFor() }.getOrNull()
                poll.interrupt()
                if (!alive(g)) break
                val ran = SystemClock.elapsedRealtime() - started
                val why = describeExit(code) + if (lastLine.isNotEmpty()) ": ${lastLine.take(160)}" else ""
                restarts++
                quick = if (ran > 60_000) 0 else quick + 1
                if (code == 128 + 4) { fail("процессор не поддерживает эту сборку (SIGILL)"); return }
                if (code == 128 + 31) { fail("Android запретил системный вызов (SIGSYS, seccomp)"); return }
                if (quick >= 5) { fail("падает при запуске ($why)"); return }
                log("exited ($why) — restart in ${backoff / 1000} s")
                state = "starting"; error = why
                backoff = if (ran > 60_000) 1000L else (backoff * 2).coerceAtMost(30_000L)
                Thread.sleep(backoff)
            }
        } catch (_: InterruptedException) {
        } finally { synchronized(lock) { if (g == gen) { proc?.destroy(); proc = null } } }
    }

    /** any HTTP answer on 127.0.0.1:port/ = listening */
    private fun pollHealth(p: Process, g: Int) {
        try {
            while (p.isAlive && g == gen && state != "ready") {
                Thread.sleep(1500)
                val ok = runCatching {
                    val c = URL("http://127.0.0.1:$port/").openConnection() as HttpURLConnection
                    c.connectTimeout = 1500; c.readTimeout = 3000
                    try { c.responseCode > 0 } finally { c.disconnect() }
                }.getOrDefault(false)
                if (ok && g == gen && state != "ready") { state = "ready"; error = null; log("ready on $host:$port") }
            }
        } catch (_: InterruptedException) { }
    }

    private fun describeExit(code: Int?): String = when {
        code == null -> "unknown"
        code > 128 -> "signal ${code - 128}" + when (code - 128) { 11 -> " (SIGSEGV)"; 4 -> " (SIGILL)"; 9 -> " (killed)"; 6 -> " (abort)"; 31 -> " (SIGSYS)"; 15 -> " (terminated)"; else -> "" }
        else -> "code $code"
    }
}

/**
 * «Голос»: whisper.cpp server (v1.9.5, accept4-patched, static musl, armv8.2+dotprod) as libwhisper_server.so on :50061.
 * Model (ggml *.bin) is downloaded from the PC like the helper's (voice_url, usually http://127.0.0.1:50055/helper/<file>).
 * API: POST /inference (multipart file=<16 kHz wav>, response_format=json) → {"text": …}.
 */
object Voice : SideServer("voice", 50061, "libwhisper_server.so", "role_voice") {
    const val PORT = 50061
    @Volatile var model: String? = null; private set
    private var url: String? = null
    private var size = 0L

    fun safeFile(f: String) = f.isNotEmpty() && f.length < 160 && (f.lowercase().endsWith(".bin") || f.lowercase().endsWith(".gguf")) &&
        !f.contains("..") && f.all { it.isLetterOrDigit() && it.code < 128 || it == '-' || it == '_' || it == '.' }

    override fun hostArg(wifi: Boolean) = if (wifi) "0.0.0.0" else "127.0.0.1"
    override fun resetTarget() { model = null }

    fun start(ctx: Context, file: String, srcUrl: String, expectSize: Long, wifi: Boolean, wantThreads: Int) = synchronized(lock) {
        if (!safeFile(file)) { model = file; fail("недопустимое имя файла модели"); return@synchronized }
        val t = if (wantThreads > 0) wantThreads else Cpu.defaultSideThreads()
        RolePrefs.save(ctx, pref, JSONObject().put("model", file).put("url", srcUrl).put("size", expectSize).put("threads", wantThreads))
        if (model == file && state != "error" && state != "idle" && host == hostArg(wifi) && t == threads) { url = srcUrl; return@synchronized }
        stopLocked()
        model = file; url = srcUrl; size = expectSize; threads = t
        log("assignment: $file from $srcUrl, -t $t")
        launch(ctx, wifi)
    }

    fun restore(ctx: Context, wifi: Boolean) {
        if (active) return
        val j = RolePrefs.load(ctx, pref) ?: return
        val m = j.optString("model").takeIf { it.isNotEmpty() } ?: return
        log("restore persisted assignment $m")
        start(ctx, m, j.optString("url"), j.optLong("size", 0L), wifi, j.optInt("threads", 0))
    }

    fun onModeChanged(ctx: Context, wifi: Boolean) {
        val m = model; val u = url
        if (active && m != null && u != null && host != hostArg(wifi)) {
            synchronized(lock) { stopLocked() }
            start(ctx, m, u, size, wifi, RolePrefs.load(ctx, pref)?.optInt("threads", 0) ?: 0)
        }
    }

    override fun args(bin: File, target: File) = Cpu.pinLittle(listOf(bin.absolutePath, "-m", target.absolutePath,
        "--host", host, "--port", PORT.toString(), "-t", threads.toString()), threads)

    override fun prepare(ctx: Context, g: Int): File? {
        val file = model ?: return null
        val dir = File(ctx.filesDir, "voice").apply { mkdirs() }
        val f = File(dir, file)
        if (f.isFile && f.length() > 0 && (size <= 0 || f.length() == size)) { pct = 100; return f }
        dir.listFiles()?.forEach { if (it.name != file && it.name != "$file.part") it.delete() }   // one voice model kept
        val src = url?.takeIf { it.isNotEmpty() } ?: throw IllegalStateException("нет адреса модели (voice_url)")
        if (size > 0) {
            val free = runCatching { StatFs(dir.absolutePath).availableBytes }.getOrDefault(Long.MAX_VALUE)
            if (free < size + (64L shl 20)) throw java.io.IOException("мало места: нужно ${size shr 20} МБ, свободно ${free shr 20} МБ")
        }
        state = "downloading"
        return Fetch.file(src, dir, file, size, { alive(g) }, { pct = it }, ::log)
    }

    fun json(): JSONObject = JSONObject().put("state", state).put("port", PORT).put("pct", pct).put("threads", threads).apply {
        model?.let { put("model", it) }; error?.let { put("error", it.take(200)) }
    }
}

/**
 * «Библиотека»: kiwix-serve 3.8.2 (static musl aarch64) as libkiwix_serve.so on :50063:
 *   `-p 50063 -i 127.0.0.1|ipv4 -a <app pid> <zim>` (-a: exits if the app process dies → no orphans).
 * .zim files live in filesDir/zim; the phone UI imports them via SAF («Выбрать .zim»).
 */
object Library : SideServer("library", 50063, "libkiwix_serve.so", "role_library") {
    const val PORT = 50063
    const val PREF = "role_library"
    @Volatile var zim: String? = null; private set
    /** SAF import progress: -1 = none, 0..100 */
    @Volatile var importPct = -1; private set
    @Volatile var importMsg: String? = null; private set

    fun dir(ctx: Context) = File(ctx.filesDir, "zim")
    fun newestZim(ctx: Context): File? = dir(ctx).listFiles { f -> f.isFile && f.name.endsWith(".zim") }?.maxByOrNull { it.lastModified() }
    fun zims(ctx: Context): List<File> = dir(ctx).listFiles { f -> f.isFile && f.name.endsWith(".zim") }?.sortedBy { it.name }.orEmpty()

    override fun hostArg(wifi: Boolean) = if (wifi) "ipv4" else "127.0.0.1"
    override fun resetTarget() { zim = null }

    private fun resolve(ctx: Context, z: String): File? {
        val f = if (z.startsWith("/")) File(z) else File(dir(ctx), z.substringAfterLast('/'))
        return f.takeIf { it.isFile && it.canRead() }
    }

    fun start(ctx: Context, z: String, wifi: Boolean) = synchronized(lock) {
        RolePrefs.save(ctx, pref, JSONObject().put("zim", z))
        if (zim == z && state != "error" && state != "idle" && host == hostArg(wifi)) return@synchronized
        stopLocked()
        zim = z
        if (resolve(ctx, z) == null) { fail("нет файла $z в ${dir(ctx).absolutePath} — выберите .zim на телефоне"); return@synchronized }
        log("assignment: $z")
        launch(ctx, wifi)
    }

    fun restore(ctx: Context, wifi: Boolean) {
        if (active) return
        val z = RolePrefs.load(ctx, pref)?.optString("zim")?.takeIf { it.isNotEmpty() } ?: return
        log("restore persisted assignment $z")
        start(ctx, z, wifi)
    }

    fun onModeChanged(ctx: Context, wifi: Boolean) {
        val z = zim
        if (active && z != null && host != hostArg(wifi)) { synchronized(lock) { stopLocked() }; start(ctx, z, wifi) }
    }

    override fun prepare(ctx: Context, g: Int): File? = zim?.let { resolve(ctx, it) ?: throw java.io.IOException("нет файла $it") }

    override fun args(bin: File, target: File) = listOf(bin.absolutePath, "-p", PORT.toString(), "-i", host,
        "-a", android.os.Process.myPid().toString(), target.absolutePath)

    fun json(): JSONObject = JSONObject().put("state", state).put("port", PORT).apply {
        zim?.let { put("zim", it) }; error?.let { put("error", it.take(200)) }
        if (importPct >= 0) put("import_pct", importPct)
    }

    /** Copy a SAF document into filesDir/zim (blocking; call off the UI thread). Returns the file name. */
    fun import(ctx: Context, uri: Uri): String {
        val cr = ctx.contentResolver
        var name = "library.zim"; var total = -1L
        cr.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
            if (c.moveToFirst()) {
                c.getString(0)?.let { name = it }
                if (!c.isNull(1)) total = c.getLong(1)
            }
        }
        name = name.replace(Regex("[^A-Za-z0-9._-]"), "_").let { if (it.lowercase().endsWith(".zim")) it else "$it.zim" }
        val d = dir(ctx).apply { mkdirs() }
        if (total > 0) {
            val free = runCatching { StatFs(d.absolutePath).availableBytes }.getOrDefault(Long.MAX_VALUE)
            if (free < total + (64L shl 20)) throw java.io.IOException("мало места: нужно ${total shr 20} МБ, свободно ${free shr 20} МБ")
        }
        val part = File(d, "$name.part"); val dst = File(d, name)
        importPct = 0; importMsg = "Копирую $name…"
        try {
            (cr.openInputStream(uri) ?: throw java.io.IOException("не открыть файл")).use { inp ->
                FileOutputStream(part).use { out ->
                    val buf = ByteArray(1 shl 20); var done = 0L
                    while (true) {
                        val n = inp.read(buf); if (n < 0) break
                        out.write(buf, 0, n); done += n
                        if (total > 0) importPct = (done * 100 / total).toInt().coerceIn(0, 99)
                        else importMsg = "Копирую $name… ${done shr 20} МБ"
                    }
                }
            }
            if (dst.exists()) dst.delete()
            if (!part.renameTo(dst)) throw java.io.IOException("не удалось сохранить $name")
            importPct = 100; importMsg = "Готово: $name (${dst.length() shr 20} МБ)"
            log("imported $name (${dst.length() shr 20} MB)")
            // the library role already runs → switch it to the new file; otherwise just remember it for role=library
            if (active) start(ctx, name, RpcService.wifi) else RolePrefs.load(ctx, pref)?.let { RolePrefs.save(ctx, pref, it.put("zim", name)) }
            return name
        } catch (e: Exception) {
            part.delete(); importPct = -1; importMsg = "Ошибка: ${e.message}"; throw e
        }
    }
}

/** Resumable HTTP download into dir/file (same behaviour as Helper's). */
object Fetch {
    fun file(src: String, dir: File, file: String, size: Long, alive: () -> Boolean, onPct: (Int) -> Unit, log: (String) -> Unit): File? {
        val part = File(dir, "$file.part"); val dst = File(dir, file)
        var tries = 0
        while (true) {
            if (!alive()) return null
            val have = if (part.exists()) part.length() else 0L
            val c = URL(src).openConnection() as HttpURLConnection
            try {
                c.connectTimeout = 8000; c.readTimeout = 30000
                if (have > 0) c.setRequestProperty("Range", "bytes=$have-")
                val code = c.responseCode
                if (code == 404) throw IllegalStateException("на ПК нет файла $file (HTTP 404)")
                if (code !in 200..299) throw IllegalStateException("ПК ответил HTTP $code")
                val append = code == 206 && have > 0
                val len = c.contentLengthLong
                val total = when { size > 0 -> size; len > 0 -> len + (if (append) have else 0); else -> 0L }
                var done = if (append) have else 0L
                log("download ${if (append) "resume at ${have shr 20} MB" else "start"} ($file, ${total shr 20} MB)")
                c.inputStream.use { inp ->
                    FileOutputStream(part, append).use { out ->
                        val buf = ByteArray(256 shl 10)
                        while (true) {
                            val n = inp.read(buf); if (n < 0) break
                            if (!alive()) return null
                            out.write(buf, 0, n); done += n
                            if (total > 0) onPct((done * 100 / total).toInt().coerceIn(0, 99))
                        }
                    }
                }
                if (total > 0 && done != total) throw java.io.IOException("загрузка оборвалась (${done shr 20} из ${total shr 20} МБ)")
                if (!part.renameTo(dst)) throw java.io.IOException("не удалось сохранить ${dst.absolutePath}")
                onPct(100); log("downloaded $file (${dst.length() shr 20} MB)")
                return dst
            } catch (e: java.io.IOException) {
                if (!alive() || ++tries > 5) throw e
                log("download interrupted (${e.message}), retry $tries/5"); Thread.sleep(2000L * tries)
            } finally { c.disconnect() }
        }
    }
}
