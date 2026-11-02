package com.harkcluster.node

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.ActivityManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.os.SystemClock
import android.net.wifi.WifiManager
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.Inet4Address
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.NetworkInterface
import java.net.ServerSocket
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import kotlin.concurrent.thread

/**
 * Foreground service that owns the ggml rpc-server process.
 * Started by the PC over adb:
 *   am start-foreground-service -n com.harkcluster.node/.RpcService --ei port 50052 --ei threads 8
 * Restarts rpc-server by itself if it exits, so a crash never needs a cable re-plug.
 *
 * Wi-Fi mode (v0.3): `--ez wifi true` (or the toggle in MainActivity, persisted in prefs "node"/"wifi_mode")
 * binds rpc-server to 0.0.0.0:50052 instead of 127.0.0.1, holds a high-perf WifiLock + MulticastLock, and
 * announces the node every 2 s by UDP broadcast to :50053 so Argus on the PC can find it on the LAN.
 * rpc-server has no auth — home network only.
 *
 * rpc-server binaries are prebuilt (scripts/build-rpc-variants-zig.sh) with the local llama.cpp patch
 * accept() -> accept4(..., SOCK_CLOEXEC): Android's app seccomp filter kills plain accept() with SIGSYS.
 */
class RpcService : Service() {
    enum class State { OFF, STARTING, READY, LINKED, RESTARTING }

    companion object {
        const val TAG = "RpcService"
        const val CHANNEL = "rpc"
        const val ACTION_STOP = "com.harkcluster.node.STOP"
        @Volatile var state = State.OFF
        @Volatile var port = 50052
        @Volatile var threads = 0
        @Volatile var startedAt = 0L          // elapsedRealtime of current rpc-server start
        @Volatile var restarts = 0
        @Volatile var clients = 0             // connections accepted since start
        @Volatile var lastExit: String? = null
        @Volatile var variant = "—"            // which rpc-server build is running (i8mm / dotprod / baseline)
        @Volatile var bigCores = 0
        @Volatile var coreTiers = ""           // e.g. "4×1.8GHz 3×2.4GHz 1×2.8GHz"
        const val VERSION_TAG = "argus-node-version"
        const val WIFI_PORT = 50052
        const val BEACON_PORT = 50053
        const val PREFS = "node"
        const val PREF_WIFI = "wifi_mode"
        @Volatile var wifi = false             // mode the running rpc-server was started in
        @Volatile var host = "127.0.0.1"
        @Volatile var wifiIp: String? = null
        @Volatile var beacons = 0L
        @Volatile private var supervisor: Thread? = null   // static: survives a service instance being recreated

        fun wifiPref(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(PREF_WIFI, false)
        fun setWifiPref(ctx: Context, on: Boolean) =
            ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(PREF_WIFI, on).apply()

        /** IPv4 of the Wi-Fi interface (wlanN, then apN) or any site-local one; with its broadcast address. */
        fun lanAddress(): Pair<Inet4Address, InetAddress?>? = runCatching {
            val all = NetworkInterface.getNetworkInterfaces().toList().filter { it.isUp && !it.isLoopback }
            val ordered = all.sortedBy { if (it.name.startsWith("wlan")) 0 else if (it.name.startsWith("ap") || it.name.startsWith("swlan")) 1 else 2 }
            for (ni in ordered) for (ia in ni.interfaceAddresses) {
                val a = ia.address
                if (a is Inet4Address && a.isSiteLocalAddress) return@runCatching a to ia.broadcast
            }
            null
        }.getOrNull()

        /** rpc-server builds shipped in jniLibs, fastest first. Each needs the listed /proc/cpuinfo Features. */
        val VARIANTS = listOf(
            Triple("i8mm", "librpc_server_i8mm.so", listOf("asimddp", "i8mm", "asimdhp")),
            Triple("dotprod", "librpc_server_dotprod.so", listOf("asimddp", "asimdhp")),
            Triple("baseline", "librpc_server.so", emptyList()),
        )
        private val badVariants = HashSet<String>()   // variants that died with SIGILL on this device

        fun cpuFeatures(): Set<String> = runCatching {
            File("/proc/cpuinfo").readLines().firstOrNull { it.startsWith("Features") }
                ?.substringAfter(":")?.trim()?.split(Regex("\\s+"))?.toSet()
        }.getOrNull() ?: emptySet()

        /** Cores whose max frequency is above the lowest tier (big + prime). All cores if there is one tier. */
        fun detectBigCores(): Int {
            val n = Runtime.getRuntime().availableProcessors()
            val freqs = (0 until 64).mapNotNull { i ->
                runCatching { File("/sys/devices/system/cpu/cpu$i/cpufreq/cpuinfo_max_freq").readText().trim().toLong() }.getOrNull()
            }
            if (freqs.isEmpty()) { coreTiers = "unknown"; return n }
            val tiers = freqs.groupingBy { it }.eachCount().toSortedMap()
            coreTiers = tiers.entries.joinToString(" ") { "${it.value}×%.1fGHz".format(it.key / 1e6) }
            if (tiers.size < 2) return freqs.size
            val lowest = tiers.firstKey()
            return freqs.count { it > lowest }.coerceAtLeast(1)
        }

        fun cacheDir(ctx: android.content.Context) = File(ctx.filesDir, "llama-cache")
        fun cacheBytes(ctx: android.content.Context): Long =
            cacheDir(ctx).walkTopDown().filter { it.isFile }.sumOf { it.length() }
        fun clearCache(ctx: android.content.Context): Boolean {
            val ok = cacheDir(ctx).deleteRecursively()
            addLog(if (ok) "weight cache cleared" else "weight cache: could not delete everything")
            return ok
        }
        val running get() = state != State.OFF
        val log = ArrayDeque<String>()
        private val ts = SimpleDateFormat("HH:mm:ss", Locale.US)
        fun addLog(line: String) = synchronized(log) {
            log.addLast("${ts.format(Date())} $line"); while (log.size > 300) log.removeFirst()
        }
        fun logText(): String = synchronized(log) { log.joinToString("\n") }
    }

    @Volatile private var proc: Process? = null
    @Volatile private var stopping = false
    @Volatile private var reconfig = false
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var mcastLock: WifiManager.MulticastLock? = null
    @Volatile private var beaconOn = false
    private var beacon: Thread? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) { stopping = true; stopSelf(); return START_NOT_STICKY }
        // explicit extra (adb: --ez wifi true|false) switches the mode and is remembered; absent = keep saved mode,
        // so the PC's plain USB start never drops a phone out of Wi-Fi mode (0.0.0.0 also covers adb forward).
        if (intent?.hasExtra("wifi") == true) setWifiPref(this, intent.getBooleanExtra("wifi", false))
        val wantWifi = wifiPref(this)
        startForegroundCompat(wantWifi)
        val v = packageManager.getPackageInfo(packageName, 0)
        val line = "$VERSION_TAG ${v.versionName} (${v.longVersionCode})"
        Log.i(TAG, line); addLog(line)

        // 0.3.2 «Помощник» (helper.rs): handled before the idempotent early return below, which would otherwise
        // swallow a helper command sent to an already running node. rpc-server handling continues unchanged.
        // 0.3.3: unified role protocol (role / role_stop / helper_* / voice_* / zim) + relaunch of persisted roles
        Roles.handleExtras(this, intent, wantWifi)
        Bench.ensure(this)

        val wantPort = if (wantWifi) WIFI_PORT else (intent?.getIntExtra("port", WIFI_PORT) ?: WIFI_PORT)
        bigCores = detectBigCores()
        // -t = big cores (little cores slow every matmul down to their pace). The PC's "threads" extra is
        // only a hint from v0.1 (it sends nproc); "force_threads" overrides the detection explicitly.
        val forced = intent?.getIntExtra("force_threads", 0) ?: 0
        val wantThreads = if (forced > 0) forced else bigCores

        // Idempotent: the PC re-runs `am start-foreground-service` on every reconnect. A live supervisor with the
        // same mode/port keeps its rpc-server (a second one would fail with "address in use").
        if (supervisorAlive()) {
            if (wantWifi == wifi && wantPort == port && !(forced > 0 && forced != threads)) {
                addLog("already running (${if (wifi) "Wi-Fi" else "USB"} $host:$port, pid alive=${proc?.isAlive == true}) → reuse")
                if (wifi) startWifiExtras()
                return START_STICKY
            }
            addLog("reconfigure: ${if (wantWifi) "Wi-Fi 0.0.0.0" else "USB 127.0.0.1"}:$wantPort t=$wantThreads → restarting rpc-server")
            applyConfig(wantWifi, wantPort, wantThreads, forced > 0)
            reconfig = true; proc?.destroy()          // supervisor relaunches immediately with the new config
            return START_STICKY
        }

        applyConfig(wantWifi, wantPort, wantThreads, forced > 0)
        stopping = false
        if (wakeLock?.isHeld != true) {
            wakeLock = (getSystemService(POWER_SERVICE) as PowerManager)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "argus:rpc").apply { acquire() }
        }
        supervisor = thread(name = "rpc-supervisor") { supervise() }
        return START_STICKY
    }

    /** `dumpsys activity service com.harkcluster.node/.RpcService` — the PC reads the `argus-helper` line (helper.rs). */
    override fun dump(fd: java.io.FileDescriptor?, writer: java.io.PrintWriter, args: Array<out String>?) {
        writer.println(Helper.dumpLine())
        writer.println("argus-status " + statusJson())   // 0.3.3: one-line JSON (telemetry + roles), see docs/ROLES.md
        writer.println("argus-rpc state=$state mode=${if (wifi) "wifi" else "usb"} port=$port threads=$threads variant=$variant clients=$clients restarts=$restarts")
        writer.println("argus-bench gbps=${Bench.cached(this)?.let { "%.2f".format(Locale.US, it) } ?: "-"} threads=${Bench.threads} running=${Bench.running}")
        writer.flush()
    }

    /** `argus-status` payload: version, telemetry, rpc and the roles map. */
    private fun statusJson(): String {
        val (vn, vc) = Updater.myVersion(this)
        val j = JSONObject().put("argus", "status").put("v", 1).put("vn", vn).put("vc", vc)
        Telemetry.put(this, j)
        wifiIp?.let { j.put("wifi_ip", it) }
        j.put("rpc", JSONObject().put("state", state.name.lowercase()).put("port", port).put("threads", threads)
            .put("big_cores", bigCores).put("variant", variant).put("mode", if (wifi) "wifi" else "usb"))
        j.put("roles", Roles.json())
        return j.toString()
    }

    private fun supervisorAlive() = supervisor?.isAlive == true && !stopping

    private fun applyConfig(w: Boolean, p: Int, t: Int, forced: Boolean) {
        wifi = w; host = if (w) "0.0.0.0" else "127.0.0.1"; port = p; threads = t
        addLog("cpu tiers: $coreTiers → threads=$threads" + if (forced) " (forced)" else " (big cores)")
        if (w) startWifiExtras() else stopWifiExtras()
        startForegroundCompat(w)
    }

    // ------------------------------------------------------------------ Wi-Fi mode

    @Suppress("DEPRECATION")
    private fun startWifiExtras() {
        val wm = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        if (wifiLock?.isHeld != true) runCatching {
            wifiLock = wm.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "argus:wifi").apply { setReferenceCounted(false); acquire() }
        }.onFailure { addLog("wifi lock: ${it.message}") }
        if (mcastLock?.isHeld != true) runCatching {
            mcastLock = wm.createMulticastLock("argus:beacon").apply { setReferenceCounted(false); acquire() }
        }.onFailure { addLog("multicast lock: ${it.message}") }
        if (beacon?.isAlive != true) {
            beaconOn = true
            beacon = thread(name = "argus-beacon", isDaemon = true) { beaconLoop() }
        }
    }

    private fun stopWifiExtras() {
        beaconOn = false; beacon?.interrupt(); beacon = null
        wifiLock?.takeIf { it.isHeld }?.release(); wifiLock = null
        mcastLock?.takeIf { it.isHeld }?.release(); mcastLock = null
        wifiIp = null
    }

    /** Every 2 s: {"argus":"node",...} to 255.255.255.255:50053 and to the subnet broadcast. */
    private fun beaconLoop() {
        val sock = runCatching { DatagramSocket().apply { broadcast = true } }
            .getOrElse { addLog("beacon socket: ${it.message}"); return }
        val am = getSystemService(ActivityManager::class.java)
        val (myVn, myVc) = Updater.myVersion(this)
        val rx = ByteArray(2048)
        var lastErr = ""
        addLog("beacon: UDP broadcast → :$BEACON_PORT every 2s")
        try {
            while (beaconOn && !stopping) {
                val lan = lanAddress()
                wifiIp = lan?.first?.hostAddress ?: dhcpIp()
                val m = ActivityManager.MemoryInfo().also { am.getMemoryInfo(it) }
                val json = JSONObject()
                    .put("argus", "node").put("v", 1).put("name", Build.MODEL).put("port", port)
                    .put("ram_mb", m.totalMem shr 20).put("free_mb", m.availMem shr 20).put("variant", variant)
                    .put("vn", myVn).put("vc", myVc)          // 0.3.1: app version for «Нода устарела» on the PC
                    .put("helper", Helper.beaconJson())       // 0.3.2: «Помощник» status (beacon.rs BeaconHelper)
                    .apply { Bench.cached(this@RpcService)?.let { put("bench", Math.round(it * 100.0) / 100.0) } }   // 0.3.2: autobench GB/s
                    .let { Telemetry.put(this@RpcService, it) }   // 0.3.3: temp_c, throttled, battery_pct, charging, usb_net_ip
                    .put("roles", Roles.json())                      // 0.3.3: {helper,voice,library:{state,port,...}}
                    .toString().toByteArray()
                val targets = linkedSetOf<InetAddress>(InetAddress.getByName("255.255.255.255"))
                (lan?.second ?: dhcpBroadcast())?.let { targets.add(it) }
                for (t in targets) {
                    val err = runCatching { sock.send(DatagramPacket(json, json.size, t, BEACON_PORT)) }.exceptionOrNull()
                    if (err == null) beacons++
                    else if ("${t.hostAddress}: ${err.message}" != lastErr) { lastErr = "${t.hostAddress}: ${err.message}"; addLog("beacon $lastErr") }
                }
                // 0.3.1: Argus on the PC answers a beacon with a unicast {"argus":"host",...} to this socket
                // (PC address + update port + versionCode of the APK it bundles). Wait for it instead of sleeping.
                val until = SystemClock.elapsedRealtime() + 2000
                while (beaconOn && !stopping) {
                    val left = until - SystemClock.elapsedRealtime(); if (left <= 0) break
                    sock.soTimeout = left.toInt().coerceAtLeast(1)
                    val pkt = DatagramPacket(rx, rx.size)
                    try { sock.receive(pkt) } catch (_: java.net.SocketTimeoutException) { break }
                    val msg = runCatching { JSONObject(String(pkt.data, 0, pkt.length, Charsets.UTF_8)) }.getOrNull() ?: continue
                    if (msg.optString("argus") != "host" || msg.optInt("v", 1) != 1) continue
                    val ip = pkt.address?.hostAddress ?: continue
                    if (Updater.pcHost != ip) addLog("PC found: $ip (update port ${msg.optInt("update_port", 50055)}, node_vc ${msg.optLong("node_vc")})")
                    Updater.onHostMessage(applicationContext, ip, msg)
                    Roles.onHostReply(applicationContext, ip, msg)    // helper/voice/library: {...} run / false stop / absent keep
                }
            }
        } catch (_: InterruptedException) {
        } finally { sock.close() }
    }

    @Suppress("DEPRECATION")
    private fun dhcp() = runCatching { (applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager).dhcpInfo }.getOrNull()
    private fun intToInet(i: Int) = InetAddress.getByAddress(byteArrayOf(i.toByte(), (i shr 8).toByte(), (i shr 16).toByte(), (i shr 24).toByte()))
    private fun dhcpIp(): String? = dhcp()?.takeIf { it.ipAddress != 0 }?.let { intToInet(it.ipAddress).hostAddress }
    private fun dhcpBroadcast(): InetAddress? = dhcp()?.takeIf { it.ipAddress != 0 && it.netmask != 0 }
        ?.let { intToInet((it.ipAddress and it.netmask) or it.netmask.inv()) }

    // ------------------------------------------------------------------ rpc-server supervision

    /** rpc-server processes of this app (same uid) left over from a killed service instance: they hold the port. */
    private fun killOrphans() {
        val me = android.os.Process.myPid(); val own = proc
        File("/proc").listFiles { f -> f.name.all(Char::isDigit) }?.forEach { d ->
            val pid = d.name.toInt(); if (pid == me) return@forEach
            val cmd = runCatching { File(d, "cmdline").readText() }.getOrNull() ?: return@forEach
            if (cmd.contains("librpc_server") && (own == null || !own.isAlive)) {
                android.os.Process.killProcess(pid); addLog("killed stale rpc-server pid $pid")
            }
        }
    }

    private fun portFree(): Boolean = runCatching {
        ServerSocket().use { it.reuseAddress = true; it.bind(InetSocketAddress(host, port)) }; true
    }.getOrDefault(false)

    private fun supervise() {
        var backoff = 1000L
        killOrphans()
        try { while (!stopping) {
            if (!portFree()) {      // something still holds the port (orphan exiting, TIME_WAIT): wait, don't double-bind
                killOrphans(); addLog("port $port busy — waiting"); state = State.RESTARTING
                repeat(10) { if (!stopping && !portFree()) Thread.sleep(500) }
            }
            val started = SystemClock.elapsedRealtime()
            val code = runOnce()
            if (stopping) break
            if (reconfig) { reconfig = false; backoff = 1000L; continue }
            lastExit = describeExit(code)
            // SIGILL = this CPU lacks an instruction the variant was built for → never use it again here
            if (code == 128 + 4 && variant != "baseline") {
                badVariants.add(variant)
                getSharedPreferences("node", MODE_PRIVATE).edit().putStringSet("bad_variants", HashSet(badVariants)).apply()
                addLog("variant $variant crashed with SIGILL → falling back")
                backoff = 500L; restarts++; continue
            }
            addLog("rpc-server exited: $lastExit — restarting")
            state = State.RESTARTING; restarts++
            // quick crashes back off, a long healthy run resets the delay
            backoff = if (SystemClock.elapsedRealtime() - started > 30_000) 1000L else (backoff * 2).coerceAtMost(15_000L)
            Thread.sleep(backoff)
        } } catch (_: InterruptedException) { }
        if (supervisor == Thread.currentThread() || supervisor == null) state = State.OFF
    }

    private fun runOnce(): Int? {
        val bin = pickBinary()
        val cache = File(cacheDir(this), "rpc").apply { mkdirs() }   // rpc-server uses $LLAMA_CACHE/rpc
        val cmd = listOf(bin.absolutePath, "-H", host, "-p", port.toString(), "-t", threads.toString(), "-c")
        addLog("exec: ${bin.name} [$variant] -H $host -p $port -t $threads -c (cache ${cache.absolutePath})")
        state = State.STARTING; clients = 0; startedAt = SystemClock.elapsedRealtime()
        val p = try {
            ProcessBuilder(cmd)
                .redirectErrorStream(true)
                .directory(filesDir)                      // app cwd "/" is unreadable; keep ggml off it
                .apply { environment()["HOME"] = filesDir.absolutePath; environment()["TMPDIR"] = cacheDir.absolutePath
                    environment()["LLAMA_CACHE"] = cacheDir(this@RpcService).absolutePath }
                .start()
        } catch (e: Exception) { addLog("cannot exec: ${e.message}"); return -1 }
        proc = p
        p.inputStream.bufferedReader().forEachLine { line ->
            if (line.contains("ggml_backend_load_best")) return@forEachLine   // harmless noise
            Log.i(TAG, line); addLog(line)
            when {
                line.contains("transport") -> state = State.READY
                line.contains("Accepted client") -> { clients++; state = State.LINKED }
                line.contains("connection closed") -> state = State.READY
            }
        }
        return runCatching { p.waitFor() }.getOrNull()
    }

    /** Fastest shipped build the CPU supports (i8mm > asimddp > baseline), minus any that SIGILLed before. */
    private fun pickBinary(): File {
        synchronized(badVariants) {
            badVariants.addAll(getSharedPreferences("node", MODE_PRIVATE).getStringSet("bad_variants", emptySet())!!)
        }
        val feats = cpuFeatures()
        val dir = applicationInfo.nativeLibraryDir
        for ((name, lib, needs) in VARIANTS) {
            val f = File(dir, lib)
            if (name in badVariants || !f.exists() || !feats.containsAll(needs)) continue
            if (variant != name) addLog("cpu variant: $name (features: ${needs.joinToString("+").ifEmpty { "armv8-a" }})")
            variant = name; return f
        }
        variant = "baseline"; return File(dir, "librpc_server.so")
    }

    private fun describeExit(code: Int?): String = when {
        code == null -> "unknown"
        code > 128 -> "signal ${code - 128}" + when (code - 128) { 11 -> " (SIGSEGV)"; 4 -> " (SIGILL)"; 9 -> " (killed)"; 6 -> " (abort)"; 15 -> " (terminated)"; else -> "" }
        else -> "code $code"
    }

    override fun onDestroy() {
        stopping = true
        proc?.destroy(); proc = null
        supervisor?.interrupt(); supervisor = null
        state = State.OFF
        stopWifiExtras()
        Helper.stop("node stopped"); Voice.stop("node stopped"); Library.stop("node stopped")   // assignments stay persisted
        wakeLock?.takeIf { it.isHeld }?.release()
        addLog("node stopped")
        super.onDestroy()
    }

    private fun startForegroundCompat(wifiMode: Boolean = wifi) {
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL, "Узел кластера", NotificationManager.IMPORTANCE_LOW))
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        val notification = Notification.Builder(this, CHANNEL)
            .setContentTitle(if (wifiMode) "Argus: узел активен (Wi-Fi)" else "Argus: узел активен")
            .setContentText(if (wifiMode) "RPC на 0.0.0.0:$WIFI_PORT — только домашняя сеть" else "Делится ОЗУ и CPU с ПК по USB")
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentIntent(open)
            .setOngoing(true)
            .build()
        if (Build.VERSION.SDK_INT >= 34) startForeground(1, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(1, notification)
    }
}
