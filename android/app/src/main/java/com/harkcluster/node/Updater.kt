package com.harkcluster.node

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.SystemClock
import android.provider.Settings
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/**
 * Node self-update (0.3.1). The user is ALWAYS asked: nothing is downloaded or installed without a tap.
 *
 * Sources, in this order:
 *  1. Argus on the PC (Wi-Fi mode): the desktop answers our UDP beacon with
 *     {"argus":"host","v":1,"update_port":50055,"node_vc":N} (unicast to the beacon's source port) and serves
 *     the APK bundled with it at http://<pc>:50055/node.version | /node.apk | /node.sha256 while it runs.
 *  2. GitHub releases/latest, asset Argus-Node.apk (+ SHA256SUMS.txt) — the «Проверить обновление» button
 *     falls back to it when no PC is known or the PC has nothing newer.
 * Install = PackageInstaller session; Android shows its own confirmation (one more tap — expected).
 */
object Updater {
    const val TAG = "ArgusUpdater"
    private const val CHANNEL = "update"
    private const val NOTIF_ID = 2
    const val EXTRA_SHOW = "show_update"
    private const val GITHUB_LATEST = "https://api.github.com/repos/femboy1488/Project-argus/releases/latest"
    private const val PREF_SKIP = "skip_update_vc"
    private const val PREF_SKIP_VN = "skip_update_vn"
    const val PREF_RUN_AFTER = "run_after_update"

    data class Avail(val vn: String, val vc: Long, val source: String, val apkUrl: String, val sha256: String?, val notes: String = "")

    @Volatile var pcHost: String? = null
    @Volatile var pcPort = 50055
    @Volatile var pcSeenAt = 0L
    @Volatile var available: Avail? = null
    @Volatile var status = ""
    @Volatile var progress = -1           // 0..100 while downloading, -1 otherwise
    @Volatile var busy = false
    @Volatile private var lastPcCheck = 0L
    @Volatile private var lastOfferedVc = 0L
    /** set when the PC says "update":true (button «Обновить по Wi-Fi»): show the question even if skipped */
    @Volatile var forced = false

    fun myVersion(ctx: Context): Pair<String, Long> {
        val p = ctx.packageManager.getPackageInfo(ctx.packageName, 0)
        return (p.versionName ?: "?") to p.longVersionCode
    }

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(RpcService.PREFS, Context.MODE_PRIVATE)
    fun skipped(ctx: Context, a: Avail): Boolean {
        val p = prefs(ctx)
        return if (a.vc > 0) p.getLong(PREF_SKIP, 0L) == a.vc else p.getString(PREF_SKIP_VN, "") == a.vn
    }
    fun skip(ctx: Context, a: Avail) {
        prefs(ctx).edit().putLong(PREF_SKIP, a.vc).putString(PREF_SKIP_VN, a.vn).apply()
        cancelNotification(ctx)
        status = "Версия ${a.vn} пропущена"
    }

    /** The service got {"argus":"host",...} from the PC. Cheap; network work runs on a thread. */
    fun onHostMessage(ctx: Context, ip: String, json: JSONObject) {
        pcHost = ip; pcPort = json.optInt("update_port", 50055); pcSeenAt = SystemClock.elapsedRealtime()
        val nodeVc = json.optLong("node_vc", 0L)
        val force = json.optBoolean("update", false)
        val (_, myVc) = myVersion(ctx)
        if (busy) return
        val due = SystemClock.elapsedRealtime() - lastPcCheck > 10 * 60_000L || nodeVc != lastOfferedVc
        if (force || (nodeVc > myVc && due)) {
            lastPcCheck = SystemClock.elapsedRealtime(); lastOfferedVc = nodeVc
            if (force) forced = true
            Thread({
                val a = runCatching { checkPc(ctx) }.onFailure { status = "ПК: ${it.message}" }.getOrNull()
                if (a != null && (force || !skipped(ctx, a))) notifyAvailable(ctx, a)
            }, "argus-update-check").start()
        }
    }

    /** Newer APK on the PC, or null. */
    fun checkPc(ctx: Context): Avail? {
        val host = pcHost ?: return null
        val base = "http://$host:$pcPort"
        val spec = httpText("$base/node.version").trim()            // "0.3.1 (5)"
        val vn = spec.substringBefore('(').trim()
        val vc = spec.substringAfter('(', "").substringBefore(')').trim().toLongOrNull() ?: 0L
        val sha = runCatching { httpText("$base/node.sha256").trim().take(64) }.getOrNull()?.takeIf { it.length == 64 }
        val (myVn, myVc) = myVersion(ctx)
        val newer = if (vc > 0) vc > myVc else semverLt(myVn, vn)
        RpcService.addLog("update check (PC $host): $spec, installed $myVn ($myVc)")
        if (!newer) { status = "ПК: установлена актуальная версия ($myVn)"; return null }
        return Avail(vn, vc, "ПК $host", "$base/node.apk", sha).also { available = it; status = "Доступна версия $vn (с ПК)" }
    }

    /** GitHub releases/latest → Argus-Node.apk, or null. */
    fun checkGithub(ctx: Context): Avail? {
        val j = JSONObject(httpText(GITHUB_LATEST))
        val tag = j.optString("tag_name")
        val vn = parseTag(tag) ?: run { status = "GitHub: тег «$tag» не похож на версию vX.Y.Z"; return null }
        val assets = j.optJSONArray("assets")
        var apk: String? = null; var sums: String? = null
        if (assets != null) for (i in 0 until assets.length()) {
            val a = assets.getJSONObject(i)
            when (a.optString("name")) {
                "Argus-Node.apk" -> apk = a.optString("browser_download_url")
                "SHA256SUMS.txt" -> sums = a.optString("browser_download_url")
            }
        }
        val (myVn, _) = myVersion(ctx)
        RpcService.addLog("update check (GitHub): $tag, installed $myVn")
        if (!semverLt(myVn, vn)) { status = "GitHub: установлена актуальная версия ($myVn)"; return null }
        if (apk == null) { status = "GitHub: в релизе $tag нет Argus-Node.apk"; return null }
        val sha = sums?.let { u -> runCatching { parseSums(httpText(u))["Argus-Node.apk"] }.getOrNull() }
        return Avail(vn, 0L, "GitHub $tag", apk, sha, j.optString("body").take(2000))
            .also { available = it; status = "Доступна версия $vn (GitHub)" }
    }

    /** Button «Проверить обновление»: PC first (if heard in the last 5 min), then GitHub. Blocking. */
    fun manualCheck(ctx: Context): Avail? {
        status = "Проверка…"
        val pcRecent = pcHost != null && SystemClock.elapsedRealtime() - pcSeenAt < 5 * 60_000L
        if (pcRecent) runCatching { checkPc(ctx) }.onFailure { status = "ПК недоступен: ${it.message}" }.getOrNull()?.let { return it }
        return runCatching { checkGithub(ctx) }.onFailure { status = "GitHub недоступен: ${it.message}" }.getOrNull()
    }

    /** Download + verify (sha256 when known, package name, newer versionCode). Blocking. Returns the file. */
    fun download(ctx: Context, a: Avail): File {
        busy = true; progress = 0; status = "Загрузка ${a.vn}…"
        try {
            val dir = File(ctx.cacheDir, "update").apply { deleteRecursively(); mkdirs() }
            val out = File(dir, "Argus-Node.apk")
            val c = open(a.apkUrl, readTimeout = 60_000)
            val total = c.contentLengthLong
            val md = MessageDigest.getInstance("SHA-256")
            c.inputStream.use { inp -> out.outputStream().use { o ->
                val buf = ByteArray(64 * 1024); var done = 0L
                while (true) {
                    val n = inp.read(buf); if (n < 0) break
                    o.write(buf, 0, n); md.update(buf, 0, n); done += n
                    if (total > 0) progress = (done * 100 / total).toInt()
                }
            } }
            val hex = md.digest().joinToString("") { "%02x".format(it) }
            if (a.sha256 != null && !a.sha256.equals(hex, ignoreCase = true)) {
                out.delete(); throw IllegalStateException("SHA-256 не совпадает — файл повреждён, установка отменена")
            }
            val info = ctx.packageManager.getPackageArchiveInfo(out.absolutePath, 0)
                ?: run { out.delete(); throw IllegalStateException("файл не является APK") }
            if (info.packageName != ctx.packageName) { out.delete(); throw IllegalStateException("чужой пакет ${info.packageName}") }
            val (_, myVc) = myVersion(ctx)
            if (info.longVersionCode <= myVc) { out.delete(); throw IllegalStateException("в файле не новее установленной (${info.versionName} / ${info.longVersionCode})") }
            RpcService.addLog("update downloaded: ${info.versionName} (${info.longVersionCode}) sha256=${hex.take(16)}… ${if (a.sha256 != null) "verified" else "no checksum published"}")
            status = "Загружено ${info.versionName}"
            return out
        } finally { busy = false; progress = -1 }
    }

    /** Hands the APK to Android's installer; Android shows its confirmation screen. Must be called while visible. */
    fun install(ctx: Context, apk: File): Boolean {
        if (!ctx.packageManager.canRequestPackageInstalls()) {
            status = "Разрешите «Установка неизвестных приложений» для Argus Node и нажмите «Обновить» ещё раз"
            runCatching {
                ctx.startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${ctx.packageName}"))
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
            return false
        }
        // restart the node after the update if it was running (MY_PACKAGE_REPLACED → UpdateReceiver)
        prefs(ctx).edit().putBoolean(PREF_RUN_AFTER, RpcService.running).apply()
        val pi = ctx.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(ctx.packageName)
            if (Build.VERSION.SDK_INT >= 31) setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_REQUIRED)
        }
        val id = pi.createSession(params)
        pi.openSession(id).use { s ->
            s.openWrite("base.apk", 0, apk.length()).use { o -> apk.inputStream().use { it.copyTo(o) }; s.fsync(o) }
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or (if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0)
            val cb = PendingIntent.getBroadcast(ctx, 7, Intent(ctx, UpdateReceiver::class.java).setAction(UpdateReceiver.ACTION_STATUS), flags)
            s.commit(cb.intentSender)
        }
        status = "Подтвердите установку в окне Android"
        cancelNotification(ctx)
        return true
    }

    // ------------------------------------------------------------------ notification (the question itself is a dialog in MainActivity)

    fun notifyAvailable(ctx: Context, a: Avail) {
        available = a
        val nm = ctx.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL, "Обновления", NotificationManager.IMPORTANCE_DEFAULT))
        val open = PendingIntent.getActivity(ctx, 3, Intent(ctx, MainActivity::class.java).putExtra(EXTRA_SHOW, true)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val n = Notification.Builder(ctx, CHANNEL)
            .setContentTitle("Доступна версия Argus Node ${a.vn}")
            .setContentText("Нажмите, чтобы решить: обновить, позже или пропустить")
            .setSmallIcon(android.R.drawable.stat_sys_download_done)
            .setContentIntent(open).setAutoCancel(true).build()
        runCatching { nm.notify(NOTIF_ID, n) }
        RpcService.addLog("update available: ${a.vn} from ${a.source} — waiting for the user")
    }
    fun cancelNotification(ctx: Context) = runCatching { ctx.getSystemService(NotificationManager::class.java).cancel(NOTIF_ID) }

    // ------------------------------------------------------------------ helpers

    private fun open(url: String, readTimeout: Int = 10_000): HttpURLConnection {
        var u = URL(url)
        repeat(5) {   // follow redirects manually (GitHub → objects.githubusercontent.com)
            val c = (u.openConnection() as HttpURLConnection).apply {
                connectTimeout = 10_000; this.readTimeout = readTimeout; instanceFollowRedirects = false
                setRequestProperty("User-Agent", "ArgusNode-Updater")
                if (u.host == "api.github.com") setRequestProperty("Accept", "application/vnd.github+json")
            }
            val code = c.responseCode
            if (code in 300..399) { u = URL(u, c.getHeaderField("Location")); c.disconnect(); return@repeat }
            if (code != 200) { c.disconnect(); throw IllegalStateException("HTTP $code ($u)") }
            return c
        }
        throw IllegalStateException("слишком много перенаправлений")
    }
    private fun httpText(url: String): String = open(url).let { c -> c.inputStream.use { it.readBytes().toString(Charsets.UTF_8) }.also { c.disconnect() } }

    /** "v0.3.1" / "0.3.1" / "v0.4" → "0.3.1" / "0.4"; anything else (e.g. "V2") → null. */
    fun parseTag(tag: String): String? {
        val t = tag.trim().removePrefix("v").removePrefix("V")
        return if (Regex("""^\d+\.\d+(\.\d+)?$""").matches(t)) t else null
    }
    fun semverLt(a: String, b: String): Boolean {
        fun p(s: String) = s.trim().split('.', '-', ' ').take(3).map { it.toIntOrNull() ?: 0 }.let { it + List(3 - it.size) { 0 } }
        val x = p(a); val y = p(b)
        for (i in 0 until 3) if (x[i] != y[i]) return x[i] < y[i]
        return false
    }
    /** sha256sum format: "<hex>  name" or "<hex> *name". */
    fun parseSums(text: String): Map<String, String> = text.lines().mapNotNull { l ->
        val m = Regex("""^([0-9a-fA-F]{64})\s+\*?(.+)$""").find(l.trim()) ?: return@mapNotNull null
        m.groupValues[2].trim().substringAfterLast('/') to m.groupValues[1].lowercase()
    }.toMap()
}

/** PackageInstaller status + MY_PACKAGE_REPLACED (restart the node service if it was running before the update). */
class UpdateReceiver : BroadcastReceiver() {
    companion object { const val ACTION_STATUS = "com.harkcluster.node.UPDATE_STATUS" }
    override fun onReceive(ctx: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_MY_PACKAGE_REPLACED) {
            val p = ctx.getSharedPreferences(RpcService.PREFS, Context.MODE_PRIVATE)
            if (p.getBoolean(Updater.PREF_RUN_AFTER, false)) {
                p.edit().putBoolean(Updater.PREF_RUN_AFTER, false).apply()
                runCatching { ctx.startForegroundService(Intent(ctx, RpcService::class.java)) }
                    .onFailure { Log.w(Updater.TAG, "restart after update: ${it.message}") }
            }
            return
        }
        when (val st = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                @Suppress("DEPRECATION")
                val confirm = intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT)
                if (confirm != null) runCatching { ctx.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
                    .onFailure { Updater.status = "Не удалось открыть окно установки: ${it.message}" }
            }
            PackageInstaller.STATUS_SUCCESS -> { Updater.status = "Обновлено"; Updater.available = null }
            else -> {
                val msg = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE) ?: "код $st"
                Updater.status = if (st == PackageInstaller.STATUS_FAILURE_ABORTED) "Установка отменена" else "Установка не удалась: $msg"
                if (st != PackageInstaller.STATUS_FAILURE_ABORTED)
                    ctx.getSharedPreferences(RpcService.PREFS, Context.MODE_PRIVATE).edit().putBoolean(Updater.PREF_RUN_AFTER, false).apply()
                RpcService.addLog("update: ${Updater.status}")
            }
        }
    }
}
