package com.agent2api.mobile;

import android.app.Activity;
import android.content.Intent;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.Build;
import android.os.SystemClock;
import android.provider.Settings;
import androidx.core.content.FileProvider;
import com.getcapacitor.JSObject;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.annotation.CapacitorPlugin;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.security.MessageDigest;
import java.util.Objects;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * 应用内更新：下载 APK 到应用私有缓存目录，再交给系统安装器。
 *
 * ── 为什么自己写而不用 @capacitor/filesystem ──────────────────────
 * 那个插件没有「下载」能力（只有读写），而 `CapacitorHttp` 会把整个响应读进
 * 内存 —— 几十 MB 的安装包会直接把 WebView 撑爆。这里用 HttpURLConnection
 * 边读边写，内存占用只有缓冲区大小。
 *
 * ── 为什么放 cacheDir 而不是外部存储 ────────────────────────────
 * 不需要任何存储权限；FileProvider 通过 `file_paths.xml` 的 `<cache-path>`
 * 已经把这个目录纳管（见 `res/xml/file_paths.xml`）。
 *
 * ── 安装权限 ────────────────────────────────────────────────────
 * Android 8.0 起「安装未知来源应用」是按应用授权的。没授权时 `startActivity`
 * 会抛 ActivityNotFoundException 或静默失败，所以先查
 * `canRequestPackageInstalls()`；没有就返回 `needsPermission: true`，
 * 由界面引导用户去系统设置页打开（见 `openInstallPermissionSettings`）。
 * 不在这里直接跳设置页 —— 那样用户不知道发生了什么就被弹出应用。
 */
@CapacitorPlugin(name = "ApkUpdater")
public class ApkUpdaterPlugin extends Plugin {

    /** 下载线程：单线程足够（同一时刻只该有一个下载任务） */
    private final ExecutorService executor = Executors.newSingleThreadExecutor();

    /** 下载中的取消标志（`download` 与 `cancel` 之间共享） */
    private final AtomicBoolean cancelled = new AtomicBoolean(false);
    private final AtomicBoolean operationActive = new AtomicBoolean(false);
    private final Set<HttpURLConnection> connections = ConcurrentHashMap.newKeySet();
    private static final long MAX_BYTES = 600L * 1024 * 1024;

    /** 目标文件：应用缓存目录下的固定名，重下时覆盖 */
    private File targetFile() {
        File dir = new File(getContext().getCacheDir(), "updates");
        if (!dir.exists()) {
            //noinspection ResultOfMethodCallIgnored
            dir.mkdirs();
        }
        return new File(dir, "agent2api-website-update.apk");
    }

    private File partialFile() {
        return new File(targetFile().getParentFile(), "agent2api-website-update.apk.part");
    }

    /**
     * 下载 APK。参数 `{ url, sha256, size }`，校验值由网关从官方 API 获取。
     * 手机自己出网，不经网关中转。
     *
     * 进度通过 `downloadProgress` 事件推送（`{received, total, percent}`），
     * 界面据此画进度条 —— 不用轮询，省一次往返。
     */
    @PluginMethod
    public void download(PluginCall call) {
        String url = call.getString("url", "");
        if (url == null || url.isEmpty()) {
            call.reject("缺少下载地址");
            return;
        }
        String expectedHash = call.getString("sha256", "");
        if (expectedHash == null || !expectedHash.matches("[0-9a-fA-F]{64}")) {
            call.reject("官网安装包缺少可信校验值");
            return;
        }
        try {
            if (!ApkUpdateConnection.isWebsiteAsset(new URL(url))) {
                call.reject("更新包只能从 Agent2API 官网下载");
                return;
            }
        } catch (Exception error) {
            call.reject("官网安装包地址无效");
            return;
        }
        final String hash = expectedHash;
        final long expectedSize = call.getData().optLong("size", 0);
        if (expectedSize <= 0 || expectedSize > MAX_BYTES) { call.reject("官网安装包大小无效"); return; }
        if (!operationActive.compareAndSet(false, true)) { call.reject("下载正在进行"); return; }
        cancelled.set(false);

        executor.execute(() -> {
            FileOutputStream output = null;
            InputStream input = null;
            HttpURLConnection connection = null;
            JSObject result = null;
            String failure = null;
            try {
                File file = partialFile();
                if (file.exists() && !file.delete()) throw new Exception("无法清理上次下载的临时文件");
                File old = targetFile();
                if (old.exists() && !old.delete()) throw new Exception("无法清理上次下载的安装包");
                if (cancelled.get()) throw new Exception("已取消下载");

                connection = ApkUpdateConnection.open(url, SystemClock.elapsedRealtime() + 12_000, cancelled, connections);
                int status = connection.getResponseCode();
                if (status == 429) throw new Exception("官网请求过于频繁，请稍后重试");
                if (status != HttpURLConnection.HTTP_OK) {
                    throw new Exception("服务器返回 HTTP " + status);
                }

                long declared = connection.getContentLengthLong();
                long total = expectedSize > 0 ? expectedSize : declared;
                if (total > MAX_BYTES || declared > MAX_BYTES) throw new Exception("安装包超过体积上限");
                input = connection.getInputStream();
                output = new FileOutputStream(file);
                MessageDigest digest = MessageDigest.getInstance("SHA-256");
                byte[] buffer = new byte[64 * 1024];
                long received = 0;
                long lastEmit = 0;
                emitProgress(0, total);
                int read;
                while ((read = input.read(buffer)) != -1) {
                    if (cancelled.get()) throw new Exception("已取消下载");
                    received += read;
                    if (received > MAX_BYTES) throw new Exception("安装包超过体积上限");
                    output.write(buffer, 0, read);
                    digest.update(buffer, 0, read);
                    long now = System.currentTimeMillis();
                    if (now - lastEmit > 200) {
                        lastEmit = now;
                        emitProgress(received, total);
                    }
                }
                output.flush();
                output.close();
                output = null;
                input.close();
                input = null;
                if (cancelled.get()) throw new Exception("已取消下载");
                if (received == 0 || (declared > 0 && received != declared)
                    || (expectedSize > 0 && received != expectedSize)) {
                    throw new Exception("安装包不完整，已拒绝使用");
                }
                if (!hash.isEmpty()) {
                    StringBuilder actual = new StringBuilder();
                    for (byte value : digest.digest()) {
                        actual.append(String.format(java.util.Locale.ROOT, "%02x", value & 0xff));
                    }
                    if (!hash.equalsIgnoreCase(actual.toString())) {
                        throw new Exception("安装包 SHA-256 与可信发布校验值不一致，已拒绝使用");
                    }
                }
                File target = targetFile();
                if (!file.renameTo(target)) throw new Exception("无法保存校验后的安装包");
                emitProgress(received, total);
                result = new JSObject();
                result.put("path", target.getAbsolutePath());
                result.put("size", received);
            } catch (Exception error) {
                failure = cancelled.get() ? "已取消下载" : "下载失败：" + error.getMessage();
            } finally {
                try { if (input != null) input.close(); } catch (Exception ignored) { }
                try { if (output != null) output.close(); } catch (Exception ignored) { }
                ApkUpdateConnection.close(connection, connections);
                partialFile().delete();
                operationActive.set(false);
            }
            if (failure != null) call.reject(failure);
            else if (result != null) call.resolve(result);
            else call.reject("下载失败");
        });
    }

    private void emitProgress(long received, long total) {
        JSObject data = new JSObject();
        data.put("received", received);
        data.put("total", total);
        data.put("percent", total > 0 ? (int) (received * 100 / total) : -1);
        notifyListeners("downloadProgress", data);
    }

    /** 取消正在进行的下载（已下好的包不动 —— 取消只针对「正在下」这个动作） */
    @PluginMethod
    public void cancel(PluginCall call) {
        cancelled.set(true);
        for (HttpURLConnection connection : connections) ApkUpdateConnection.close(connection, connections);
        call.resolve();
    }

    private void startActivitySafely(Intent intent, boolean forceNewTask) {
        Activity activity = getActivity();
        if (activity != null && !activity.isFinishing() && !activity.isDestroyed()) {
            if (forceNewTask) {
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            }
            activity.startActivity(intent);
        } else {
            intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            getContext().startActivity(intent);
        }
    }

    @SuppressWarnings("deprecation")
    private PackageInfo getInstalledPackageInfo() throws PackageManager.NameNotFoundException {
        PackageManager pm = getContext().getPackageManager();
        String pkg = getContext().getPackageName();
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            return pm.getPackageInfo(pkg, PackageManager.PackageInfoFlags.of(0));
        }
        return pm.getPackageInfo(pkg, 0);
    }

    @SuppressWarnings("deprecation")
    private PackageInfo getArchivePackageInfo(File file) {
        PackageManager pm = getContext().getPackageManager();
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            return pm.getPackageArchiveInfo(file.getAbsolutePath(), PackageManager.PackageInfoFlags.of(0));
        }
        return pm.getPackageArchiveInfo(file.getAbsolutePath(), 0);
    }

    @SuppressWarnings("deprecation")
    private static long versionCodeOf(PackageInfo info) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            return info.getLongVersionCode();
        }
        return info.versionCode;
    }

    /**
     * 校验缓存目录里的 APK 是否仍然是有效的「待安装更新」。
     *
     * Android 覆盖升级后不会自动清空 cacheDir；若不比对版本，升级完成后
     * 重进设置页还会一直停留在「新版本安装包已就绪」。损坏、旧版或已安装的
     * 同版本包在这里顺手清理。
     */
    private boolean hasPendingUpdateApk(File file) {
        if (!file.exists() || file.length() <= 0) {
            if (file.exists()) {
                //noinspection ResultOfMethodCallIgnored
                file.delete();
            }
            return false;
        }
        try {
            PackageInfo archive = getArchivePackageInfo(file);
            if (archive == null || archive.packageName == null) {
                //noinspection ResultOfMethodCallIgnored
                file.delete();
                return false;
            }
            if (getContext().getPackageName().equals(archive.packageName)) {
                PackageInfo current = getInstalledPackageInfo();
                long archiveCode = versionCodeOf(archive);
                long currentCode = versionCodeOf(current);
                if (archiveCode < currentCode
                    || (archiveCode == currentCode && Objects.equals(archive.versionName, current.versionName))) {
                    //noinspection ResultOfMethodCallIgnored
                    file.delete();
                    return false;
                }
            }
            return true;
        } catch (Exception ignored) {
            return file.exists() && file.length() > 0;
        }
    }

    /**
     * 当前是否允许安装未知来源应用。
     *
     * Android 8.0 以下没有这个概念（默认允许），直接返回 true —— 那些版本
     * 由系统安装界面自己提示。
     */
    @PluginMethod
    public void canInstall(PluginCall call) {
        try {
            JSObject result = new JSObject();
            boolean allowed = true;
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                allowed = getContext().getPackageManager().canRequestPackageInstalls();
            }
            result.put("allowed", allowed);
            call.resolve(result);
        } catch (Exception error) {
            call.reject("无法检查安装权限：" + error.getMessage());
        }
    }

    /** 跳到本应用的「安装未知应用」设置页（`canInstall` 为 false 时由界面调用） */
    @PluginMethod
    public void openInstallPermissionSettings(PluginCall call) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            call.resolve();
            return;
        }
        Uri packageUri = Uri.parse("package:" + getContext().getPackageName());
        Intent[] candidates = new Intent[] {
            new Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, packageUri),
            new Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES),
            new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, packageUri),
        };
        Exception lastError = null;
        for (Intent candidate : candidates) {
            try {
                startActivitySafely(candidate, false);
                call.resolve();
                return;
            } catch (Exception error) {
                lastError = error;
            }
        }
        call.reject("无法打开安装权限设置：" + (lastError != null ? lastError.getMessage() : "系统未提供对应页面"));
    }

    /**
     * 调起系统安装器。
     *
     * 用 FileProvider 生成 `content://` URI：Android 7.0 起直接传 `file://`
     * 会抛 FileUriExposedException。`FLAG_GRANT_READ_URI_PERMISSION` 把临时读
     * 权限授给安装器进程 —— 少了这个，安装器打不开文件、表现为「解析包出错」。
     */
    @PluginMethod
    public void install(PluginCall call) {
        File file = targetFile();
        if (!file.exists() || file.length() <= 0) {
            if (file.exists()) {
                //noinspection ResultOfMethodCallIgnored
                file.delete();
            }
            call.reject("安装包不存在，请先下载");
            return;
        }
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O
                && !getContext().getPackageManager().canRequestPackageInstalls()) {
                call.reject("尚未授予「安装未知来源应用」权限，请先授权后再试");
                return;
            }
            PackageInfo archive = null;
            boolean archiveChecked = false;
            try {
                archive = getArchivePackageInfo(file);
                archiveChecked = true;
            } catch (Exception ignored) {
                // 个别定制 ROM 解析异常时不阻断系统安装器
            }
            if (archiveChecked && (archive == null || archive.packageName == null)) {
                //noinspection ResultOfMethodCallIgnored
                file.delete();
                call.reject("安装包已损坏，请重新下载");
                return;
            }
            Uri uri = FileProvider.getUriForFile(
                getContext(),
                getContext().getPackageName() + ".fileprovider",
                file
            );
            Intent intent = new Intent(Intent.ACTION_VIEW);
            intent.setDataAndType(uri, "application/vnd.android.package-archive");
            intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
            startActivitySafely(intent, true);
            call.resolve();
        } catch (Exception error) {
            call.reject("无法启动安装器：" + error.getMessage());
        }
    }

    /** 当前已下载的安装包信息（重进设置页时据此显示「已下载，待安装」） */
    @PluginMethod
    public void downloaded(PluginCall call) {
        File file = targetFile();
        boolean pending = hasPendingUpdateApk(file);
        JSObject result = new JSObject();
        result.put("exists", pending);
        result.put("size", pending ? file.length() : 0);
        call.resolve(result);
    }

    @Override
    protected void handleOnDestroy() {
        cancelled.set(true);
        for (HttpURLConnection connection : connections) ApkUpdateConnection.close(connection, connections);
        executor.shutdownNow();
        super.handleOnDestroy();
    }
}
