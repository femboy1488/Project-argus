package com.harkcluster.node

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import kotlin.concurrent.thread

/**
 * Foreground service that owns the ggml rpc-server process.
 * Started by the PC over adb:
 *   am start-foreground-service -n com.harkcluster.node/.RpcService --ei port 50052 --ei threads 8
 * Restarts rpc-server by itself if it exits, so a crash never needs a cable re-plug.
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

    private var proc: Process? = null
    @Volatile private var stopping = false
    private var wakeLock: PowerManager.WakeLock? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) { stopping = true; stopSelf(); return START_NOT_STICKY }
        startForegroundCompat()
        port = intent?.getIntExtra("port", 50052) ?: 50052
        bigCores = detectBigCores()
        // -t = big cores (little cores slow every matmul down to their pace). The PC's "threads" extra is
        // only a hint from v0.1 (it sends nproc); "force_threads" overrides the detection explicitly.
        val forced = intent?.getIntExtra("force_threads", 0) ?: 0
        threads = if (forced > 0) forced else bigCores
        val v = packageManager.getPackageInfo(packageName, 0)
        val line = "$VERSION_TAG ${v.versionName} (${v.longVersionCode})"
        Log.i(TAG, line); addLog(line)
        addLog("cpu tiers: $coreTiers → threads=$threads" + if (forced > 0) " (forced)" else " (big cores)")
        if (proc?.isAlive == true) return START_STICKY   // idempotent: PC may call again on reconnect

        stopping = false
        if (wakeLock?.isHeld != true) {
            wakeLock = (getSystemService(POWER_SERVICE) as PowerManager)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "argus:rpc").apply { acquire() }
        }
        thread(name = "rpc-supervisor") { supervise() }
        return START_STICKY
    }

    private fun supervise() {
        var backoff = 1000L
        while (!stopping) {
            val started = SystemClock.elapsedRealtime()
            val code = runOnce()
            if (stopping) break
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
        }
        state = State.OFF
    }

    private fun runOnce(): Int? {
        val bin = pickBinary()
        val cache = File(cacheDir(this), "rpc").apply { mkdirs() }   // rpc-server uses $LLAMA_CACHE/rpc
        val cmd = listOf(bin.absolutePath, "-H", "127.0.0.1", "-p", port.toString(), "-t", threads.toString(), "-c")
        addLog("exec: ${bin.name} [$variant] -p $port -t $threads -c (cache ${cache.absolutePath})")
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
        state = State.OFF
        wakeLock?.takeIf { it.isHeld }?.release()
        addLog("node stopped")
        super.onDestroy()
    }

    private fun startForegroundCompat() {
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL, "Cluster node", NotificationManager.IMPORTANCE_LOW))
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        val notification = Notification.Builder(this, CHANNEL)
            .setContentTitle("Argus node active")
            .setContentText("Sharing RAM and CPU with your PC over USB")
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentIntent(open)
            .setOngoing(true)
            .build()
        if (Build.VERSION.SDK_INT >= 34) startForeground(1, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(1, notification)
    }
}
