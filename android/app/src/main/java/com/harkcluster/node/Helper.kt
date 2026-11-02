package com.harkcluster.node

import android.content.Context
import android.os.StatFs
import android.os.SystemClock
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.net.HttpURLConnection
import java.net.URL

/**
 * 0.3.2 «Помощник» (desktop helper.rs): this node runs its OWN llama-server with a small model next to rpc-server.
 *
 * Command sources:
 *  - USB: `am start-foreground-service … --es helper_model <file> --es helper_url http://127.0.0.1:50055/helper/<file>
 *    --el helper_size N` (stop: `--ez helper_stop true`); the PC reads progress from `dumpsys activity service
 *    …/.RpcService`, line `argus-helper state=… pct=… model=… error=…`.
 *  - Wi-Fi: `"helper":{"file","path","size","port"}` / `"helper":false` in the PC's reply to our beacon; status goes
 *    back in the beacon as `"helper":{"state","model","pct","error"}`.
 * The model is downloaded into filesDir/models (fallback /data/local/tmp/argus), then
 * `libllama_server.so -m <model> --host 127.0.0.1|0.0.0.0 --port 50062 -c 4096 -t <helper threads> --jinja`
 * is supervised and restarted with backoff when it crashes.
 *
 * 0.3.3: the assignment (model, url, size, threads, source) is persisted in prefs "node"/"role_helper" and relaunched
 * when the service is recreated; any stop command (helper_stop, role_stop, role=idle, beacon `"helper":false`)
 * stops it whatever source started it and forgets it. Threads default to the LITTLE cores (min 2) so rpc-server keeps
 * the big ones; when `toybox taskset` works the process is pinned to the little cores (best effort).
 */
object Helper {
    const val TAG = "ArgusHelper"
    const val PORT = 50062
    private const val LIB = "libllama_server.so"
    private const val FALLBACK_DIR = "/data/local/tmp/argus"

    /** idle | downloading | starting | ready | error */
    @Volatile var state = "idle"; private set
    @Volatile var pct = 0; private set
    @Volatile var model: String? = null; private set
    @Volatile var error: String? = null; private set
    @Volatile var host = "127.0.0.1"; private set
    @Volatile var restarts = 0; private set
    @Volatile var readyAt = 0L; private set

    private val lock = Any()
    @Volatile private var gen = 0                 // bumped on every (re)start/stop: old threads notice and quit
    @Volatile private var proc: Process? = null
    private var worker: Thread? = null
    private var url: String? = null
    private var size = 0L
    /** assignment came from the Wi-Fi beacon reply (only such a helper is stopped by `"helper": false`) */
    @Volatile private var fromLan = false
    /** -t for llama-server; 0 = default (little cores, min 2) */
    @Volatile var threads = 0; private set
    private const val PREF = "role_helper"

    val active get() = state != "idle"

    private fun log(s: String) { Log.i(TAG, s); RpcService.addLog("helper: $s") }

    /** Safe plain file name only (same rule as the PC's safe_file). */
    fun safeFile(f: String) = f.isNotEmpty() && f.length < 160 && f.lowercase().endsWith(".gguf") && !f.contains("..") &&
        f.all { it.isLetterOrDigit() && it.code < 128 || it == '-' || it == '_' || it == '.' }

    /**
     * Run [file] from [srcUrl]. Idempotent: the PC repeats the command (USB every 60 s without progress, Wi-Fi every
     * beacon reply), so the same model that is downloading/starting/ready is left alone; the same model in «error»
     * is retried.
     */
    fun start(ctx: Context, file: String, srcUrl: String, expectSize: Long, wifi: Boolean, fromLan: Boolean, wantThreads: Int = 0) {
        val app = ctx.applicationContext
        synchronized(lock) {
            if (!safeFile(file)) { setError(file, "недопустимое имя файла модели"); return }
            val wantHost = if (wifi) "0.0.0.0" else "127.0.0.1"
            val t = if (wantThreads > 0) wantThreads else Cpu.defaultSideThreads()
            if (model == file && state != "error" && state != "idle" && wantHost == host && t == threads) {
                url = srcUrl; if (expectSize > 0) size = expectSize; this.fromLan = fromLan
                persist(app, wantThreads)
                return
            }
            stopLocked("new assignment")
            model = file; url = srcUrl; size = expectSize; host = wantHost; this.fromLan = fromLan; threads = t
            persist(app, wantThreads)
            error = null; pct = 0; restarts = 0; state = "starting"
            val g = ++gen
            log("assignment: $file from $srcUrl (${if (expectSize > 0) "${expectSize shr 20} MB" else "size ?"}) on $wantHost:$PORT")
            worker = Thread({ run(app, g) }, "argus-helper").also { it.isDaemon = true; it.start() }
        }
    }

    /** [forget] = a role-stop command: also drop the persisted assignment so a service restart does not relaunch it. */
    fun stop(why: String, forget: Boolean = false, ctx: Context? = null) = synchronized(lock) {
        if (forget && ctx != null) RolePrefs.clear(ctx, PREF)
        if (active || proc != null) { stopLocked(why); log("stopped ($why)") }
    }

    private fun persist(ctx: Context, wantThreads: Int) {
        val m = model ?: return
        RolePrefs.save(ctx, PREF, JSONObject().put("model", m).put("url", url ?: "").put("size", size)
            .put("threads", wantThreads).put("lan", fromLan))
    }

    /** Service (re)start: relaunch the persisted assignment if nothing runs. */
    fun restore(ctx: Context, wifi: Boolean) {
        if (active) return
        val j = RolePrefs.load(ctx, PREF) ?: return
        val m = j.optString("model"); val u = j.optString("url")
        if (m.isEmpty()) return
        log("restore persisted assignment $m")
        start(ctx, m, u, j.optLong("size", 0L), wifi, j.optBoolean("lan", false), j.optInt("threads", 0))
    }

    /** Wi-Fi mode switched while the helper runs: restart it on the new address. */
    fun onModeChanged(ctx: Context, wifi: Boolean) {
        val m = model; val u = url
        if (active && m != null && u != null && host != (if (wifi) "0.0.0.0" else "127.0.0.1"))
            start(ctx, m, u, size, wifi, fromLan, RolePrefs.load(ctx, PREF)?.optInt("threads", 0) ?: 0)
    }

    private fun stopLocked(why: String) {
        gen++
        proc?.destroy(); proc = null
        worker?.interrupt(); worker = null
        state = "idle"; pct = 0; error = null; model = null; readyAt = 0
        if (why.isNotEmpty()) Log.i(TAG, "stop: $why")
    }

    private fun setError(file: String?, msg: String) { model = file ?: model; error = msg; state = "error"; log("error: $msg") }

    /** Applies the PC's beacon reply: object = run this, false = stop, absent/null = no change. */
    fun onHostReply(ctx: Context, pcIp: String, msg: JSONObject) {
        if (!msg.has("helper") || msg.isNull("helper")) return
        when (val h = msg.opt("helper")) {
            // 0.3.3: stop from ANY source (0.3.2 only stopped a helper the beacon had started → USB-started one stuck)
            is Boolean -> if (!h) { if (active) stop("PC (Wi-Fi): role removed", forget = true, ctx = ctx) else RolePrefs.clear(ctx, PREF) }
            is JSONObject -> {
                val file = h.optString("file"); val path = h.optString("path", "/helper/$file")
                if (file.isEmpty()) return
                val port = msg.optInt("update_port", 50055)
                start(ctx, file, "http://$pcIp:$port$path", h.optLong("size", 0L), true, fromLan = true, wantThreads = h.optInt("threads", 0))
            }
        }
    }

    /** `{"state","model","pct","error"}` for the beacon (beacon.rs BeaconHelper). */
    fun beaconJson(): JSONObject = JSONObject().put("state", state).put("pct", pct).put("port", PORT).put("threads", threads).apply {
        model?.let { put("model", it) }; error?.let { put("error", it.take(200)) }
    }

    /** The line helper.rs parse_dump() reads (values: spaces → %20, empty → "-"). */
    fun dumpLine(): String {
        fun enc(s: String?) = s?.replace("\n", " ")?.trim()?.replace(" ", "%20")?.ifEmpty { null } ?: "-"
        return "argus-helper state=$state pct=$pct model=${enc(model)} error=${enc(error)}"
    }

    // ------------------------------------------------------------------ worker

    private fun alive(g: Int) = g == gen && !Thread.currentThread().isInterrupted

    private fun run(ctx: Context, g: Int) {
        val file = model ?: return
        val f = try { ensureModel(ctx, file, g) } catch (_: InterruptedException) { return }
            catch (e: Exception) { if (alive(g)) synchronized(lock) { if (g == gen) setError(file, e.message ?: e.toString()) }; return }
        if (f == null || !alive(g)) return
        supervise(ctx, f, g)
    }

    private fun modelDirs(ctx: Context) = listOf(File(ctx.filesDir, "models"), File(FALLBACK_DIR))

    /** Existing complete copy, or download it. Returns null when cancelled. */
    private fun ensureModel(ctx: Context, file: String, g: Int): File? {
        for (d in modelDirs(ctx)) {
            val f = File(d, file)
            if (f.isFile && f.length() > 0 && (size <= 0 || f.length() == size) && f.canRead()) { pct = 100; return f }
        }
        // only one helper model is kept: drop older ones from our private dir
        File(ctx.filesDir, "models").listFiles()?.forEach { if (it.name != file && it.name != "$file.part") it.delete() }
        val src = url ?: throw IllegalStateException("нет адреса модели")
        state = "downloading"; pct = 0
        var lastErr: Exception? = null
        for (d in modelDirs(ctx)) {
            try {
                if (!d.isDirectory && !d.mkdirs()) throw java.io.IOException("нет доступа к ${d.absolutePath}")
                if (size > 0) {
                    val free = runCatching { StatFs(d.absolutePath).availableBytes }.getOrDefault(Long.MAX_VALUE)
                    val have = File(d, "$file.part").length()
                    if (free < size - have + (64L shl 20))
                        throw java.io.IOException("мало места: нужно ${(size - have) shr 20} МБ, свободно ${free shr 20} МБ")
                }
                return download(src, d, file, g)
            } catch (e: InterruptedException) { throw e
            } catch (e: Exception) {
                if (!alive(g)) return null
                lastErr = e; log("download to ${d.absolutePath}: ${e.message}")
                if (e is HttpError) break   // the PC answered: another dir won't help
            }
        }
        throw java.io.IOException("не удалось скачать модель: ${lastErr?.message}")
    }

    private class HttpError(msg: String) : java.io.IOException(msg)

    private fun download(src: String, dir: File, file: String, g: Int): File? {
        val part = File(dir, "$file.part"); val dst = File(dir, file)
        var tries = 0
        while (true) {
            if (!alive(g)) return null
            val have = if (part.exists()) part.length() else 0L
            val c = URL(src).openConnection() as HttpURLConnection
            try {
                c.connectTimeout = 8000; c.readTimeout = 30000
                if (have > 0) c.setRequestProperty("Range", "bytes=$have-")
                val code = c.responseCode
                if (code == 404) throw HttpError("на ПК нет файла $file (HTTP 404)")
                if (code !in 200..299) throw HttpError("ПК ответил HTTP $code")
                val append = code == 206 && have > 0
                val len = c.contentLengthLong
                val total = when { size > 0 -> size; len > 0 -> len + (if (append) have else 0); else -> 0L }
                var done = if (append) have else 0L
                log("download ${if (append) "resume at ${have shr 20} MB" else "start"} → ${dir.absolutePath} (${total shr 20} MB)")
                c.inputStream.use { inp ->
                    FileOutputStream(part, append).use { out ->
                        val buf = ByteArray(256 shl 10)
                        var last = 0L
                        while (true) {
                            val n = inp.read(buf); if (n < 0) break
                            if (!alive(g)) return null
                            out.write(buf, 0, n); done += n
                            if (total > 0) pct = (done * 100 / total).toInt().coerceIn(0, 99)
                            val now = SystemClock.elapsedRealtime()
                            if (now - last > 10_000) { last = now; Log.i(TAG, "download $pct%") }
                        }
                    }
                }
                if (total > 0 && done != total) throw java.io.IOException("загрузка оборвалась (${done shr 20} из ${total shr 20} МБ)")
                if (!part.renameTo(dst)) throw java.io.IOException("не удалось сохранить ${dst.absolutePath}")
                pct = 100; log("downloaded ${dst.name} (${dst.length() shr 20} MB)")
                return dst
            } catch (e: HttpError) { throw e
            } catch (e: java.io.IOException) {
                // network hiccup (Wi-Fi, cable): resume a few times before giving up
                if (!alive(g) || ++tries > 5) throw e
                log("download interrupted (${e.message}), retry $tries/5"); Thread.sleep(2000L * tries)
            } finally { c.disconnect() }
        }
    }

    /** llama-server processes of this app left over from a killed service instance: they hold :50062. */
    private fun killOrphans() {
        val me = android.os.Process.myPid(); val own = proc
        File("/proc").listFiles { f -> f.name.all(Char::isDigit) }?.forEach { d ->
            val pid = d.name.toInt(); if (pid == me) return@forEach
            val cmd = runCatching { File(d, "cmdline").readText() }.getOrNull() ?: return@forEach
            if (cmd.contains(LIB) && (own == null || !own.isAlive)) { android.os.Process.killProcess(pid); log("killed stale llama-server pid $pid") }
        }
    }

    private fun supervise(ctx: Context, model: File, g: Int) {
        val bin = File(ctx.applicationInfo.nativeLibraryDir, LIB)
        if (!bin.exists()) { synchronized(lock) { if (g == gen) setError(model.name, "в этой сборке нет llama-server ($LIB)") }; return }
        var backoff = 1000L; var quick = 0
        killOrphans()
        try {
            while (alive(g)) {
                val t = threads.takeIf { it > 0 } ?: Cpu.defaultSideThreads()
                val cmd = Cpu.pinLittle(listOf(bin.absolutePath, "-m", model.absolutePath, "--host", host, "--port", PORT.toString(),
                    "-c", "4096", "-t", t.toString(), "--jinja"), t)
                log("exec: ${if (cmd.first() != bin.absolutePath) "taskset ${cmd[2]} " else ""}$LIB -m ${model.name} --host $host --port $PORT -c 4096 -t $t --jinja")
                state = "starting"; error = null
                val started = SystemClock.elapsedRealtime()
                val p = try {
                    ProcessBuilder(cmd).redirectErrorStream(true).directory(ctx.filesDir)
                        .apply { environment()["HOME"] = ctx.filesDir.absolutePath; environment()["TMPDIR"] = ctx.cacheDir.absolutePath }
                        .start()
                } catch (e: Exception) { synchronized(lock) { if (g == gen) setError(model.name, "не запускается llama-server: ${e.message}") }; return }
                synchronized(lock) { if (g != gen) { p.destroy(); return }; proc = p }
                val poll = Thread({ pollHealth(p, g) }, "argus-helper-health").apply { isDaemon = true; start() }
                var lastLine = ""
                p.inputStream.bufferedReader().forEachLine { line ->
                    if (line.isBlank()) return@forEachLine
                    lastLine = line
                    if (line.contains("error", true) || line.contains("listening") || line.contains("model loaded") || line.contains("failed", true))
                        log(line.take(300))
                    else Log.d(TAG, line)
                    if (line.contains("starting the main loop") || line.contains("model loaded")) markReady(g)
                }
                val code = runCatching { p.waitFor() }.getOrNull()
                poll.interrupt()
                if (!alive(g)) break
                val ran = SystemClock.elapsedRealtime() - started
                val why = describeExit(code) + if (lastLine.isNotEmpty()) ": ${lastLine.take(160)}" else ""
                restarts++
                quick = if (ran > 60_000) 0 else quick + 1
                if (code == 128 + 4) { setError(model.name, "процессор не поддерживает эту сборку llama-server (SIGILL)"); return }
                if (quick >= 5) { setError(model.name, "llama-server падает при запуске ($why)"); return }
                log("llama-server exited ($why) — restart in ${backoff / 1000} s")
                state = "starting"; error = why
                backoff = if (ran > 60_000) 1000L else (backoff * 2).coerceAtMost(30_000L)
                Thread.sleep(backoff)
            }
        } catch (_: InterruptedException) { }
        finally { synchronized(lock) { if (g == gen) { proc?.destroy(); proc = null } } }
    }

    private fun markReady(g: Int) {
        if (g == gen && state != "ready") { state = "ready"; error = null; readyAt = SystemClock.elapsedRealtime(); log("ready on $host:$PORT") }
    }

    /** GET /health every 2 s until 200 (llama-server answers 503 while the model loads). */
    private fun pollHealth(p: Process, g: Int) {
        try {
            while (p.isAlive && g == gen && state != "ready") {
                Thread.sleep(2000)
                val ok = runCatching {
                    val c = URL("http://127.0.0.1:$PORT/health").openConnection() as HttpURLConnection
                    c.connectTimeout = 1500; c.readTimeout = 1500
                    try { c.responseCode == 200 } finally { c.disconnect() }
                }.getOrDefault(false)
                if (ok) markReady(g)
            }
        } catch (_: InterruptedException) { }
    }

    private fun describeExit(code: Int?): String = when {
        code == null -> "unknown"
        code > 128 -> "signal ${code - 128}" + when (code - 128) { 11 -> " (SIGSEGV)"; 4 -> " (SIGILL)"; 9 -> " (killed)"; 6 -> " (abort)"; 15 -> " (terminated)"; else -> "" }
        else -> "code $code"
    }
}

/** 0.3.2 autobench: argus-bench shipped as libargus_bench(_dotprod).so, run once per app version, result cached. */
object Bench {
    private const val P_VC = "bench_vc"; private const val P_GBPS = "bench_gbps"; private const val P_THREADS = "bench_threads"
    @Volatile var gbps: Float? = null; private set
    @Volatile var threads = 0; private set
    @Volatile var running = false; private set
    @Volatile var error: String? = null; private set

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(RpcService.PREFS, Context.MODE_PRIVATE)

    /** Cached result for this app version (loads prefs on first use). */
    fun cached(ctx: Context): Float? {
        gbps?.let { return it }
        val p = prefs(ctx); val (_, vc) = Updater.myVersion(ctx)
        if (p.getLong(P_VC, -1L) != vc) return null
        val g = p.getFloat(P_GBPS, 0f); if (g <= 0f) return null
        threads = p.getInt(P_THREADS, 0); gbps = g; return g
    }

    /** Runs the test in the background if this app version has no result yet. Skipped while the PC is linked. */
    fun ensure(ctx: Context) {
        val app = ctx.applicationContext
        if (cached(app) != null || running) return
        running = true
        Thread({
            try {
                Thread.sleep(3000)   // let rpc-server settle first
                if (RpcService.state == RpcService.State.LINKED) { RpcService.addLog("bench: PC linked — later"); return@Thread }
                val r = runBench(app)
                if (r != null) {
                    val (_, vc) = Updater.myVersion(app)
                    prefs(app).edit().putLong(P_VC, vc).putFloat(P_GBPS, r.first).putInt(P_THREADS, r.second).apply()
                    gbps = r.first; threads = r.second; error = null
                    RpcService.addLog("bench: %.2f GB/s (%d threads)".format(r.first, r.second))
                }
            } catch (_: InterruptedException) {
            } finally { running = false }
        }, "argus-bench").apply { isDaemon = true; start() }
    }

    private fun runBench(ctx: Context): Pair<Float, Int>? {
        val dir = ctx.applicationInfo.nativeLibraryDir
        val dot = "asimddp" in RpcService.cpuFeatures()
        val bins = (if (dot) listOf("libargus_bench_dotprod.so", "libargus_bench.so") else listOf("libargus_bench.so"))
            .map { File(dir, it) }.filter { it.exists() }
        if (bins.isEmpty()) { error = "нет argus-bench в сборке"; RpcService.addLog("bench: binary missing"); return null }
        for (b in bins) {
            val out = runCatching {
                val p = ProcessBuilder(b.absolutePath, "-s", "1.5").redirectErrorStream(true).directory(ctx.filesDir).start()
                val text = p.inputStream.bufferedReader().readText()
                p.waitFor(); text
            }.onFailure { error = it.message; RpcService.addLog("bench ${b.name}: ${it.message}") }.getOrNull() ?: continue
            val line = out.lineSequence().firstOrNull { it.contains("\"argus\":\"bench\"") }
            val j = line?.let { runCatching { JSONObject(it.trim()) }.getOrNull() }
            val g = j?.optDouble("gbps", 0.0)?.toFloat() ?: 0f
            if (g > 0f && g.isFinite()) return g to j!!.optInt("threads", 0)
            error = out.trim().take(160); RpcService.addLog("bench ${b.name}: unexpected output: ${error}")
        }
        return null
    }
}
