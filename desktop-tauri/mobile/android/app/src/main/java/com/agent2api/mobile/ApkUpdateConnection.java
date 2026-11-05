package com.agent2api.mobile;

import android.os.SystemClock;
import java.net.HttpURLConnection;
import java.net.URL;
import java.util.Set;
import java.util.concurrent.atomic.AtomicBoolean;

final class ApkUpdateConnection {
    static boolean isGithubAsset(URL url) {
        String host = url.getHost().toLowerCase(java.util.Locale.ROOT);
        return "https".equals(url.getProtocol()) && (url.getPort() == -1 || url.getPort() == 443)
            && (host.endsWith(".githubusercontent.com")
                || ((host.equals("github.com") || host.equals("www.github.com"))
                    && url.getPath().contains("/releases/download/")));
    }

    static HttpURLConnection open(String startUrl, String hash, long deadline, boolean probe,
        AtomicBoolean cancelled, Set<HttpURLConnection> connections) throws Exception {
        URL origin = new URL(startUrl);
        if (!"https".equals(origin.getProtocol()) || origin.getUserInfo() != null) {
            throw new Exception("安装包下载必须使用无凭证的 HTTPS 地址");
        }
        if (!isGithubAsset(origin) && hash.isEmpty()) {
            throw new Exception("加速下载缺少 GitHub 官方校验值");
        }
        URL target = origin;
        for (int hop = 0; hop < 5; hop++) {
            if (cancelled.get() || Thread.currentThread().isInterrupted()) throw new Exception("已取消下载");
            boolean sameOrigin = target.getHost().equalsIgnoreCase(origin.getHost())
                && port(target) == port(origin);
            if (!"https".equals(target.getProtocol()) || target.getUserInfo() != null
                || (!sameOrigin && !isGithubAsset(target))) {
                throw new Exception("下载源重定向到未允许的地址");
            }
            HttpURLConnection connection = (HttpURLConnection) target.openConnection();
            connections.add(connection);
            try {
                connection.setRequestMethod("GET");
                connection.setConnectTimeout(remaining(deadline));
                connection.setReadTimeout(remaining(deadline));
                connection.setInstanceFollowRedirects(false);
                connection.setRequestProperty("User-Agent", "agent2api-mobile");
                connection.setRequestProperty("Accept", "application/octet-stream");
                connection.setRequestProperty("Accept-Encoding", "identity");
                if (probe) connection.setRequestProperty("Range", "bytes=0-65535");
                int status = connection.getResponseCode();
                if (status == 301 || status == 302 || status == 303 || status == 307 || status == 308) {
                    String location = connection.getHeaderField("Location");
                    if (location == null || location.isEmpty()) throw new Exception("重定向缺少 Location");
                    target = new URL(target, location);
                    close(connection, connections);
                    continue;
                }
                if (!probe) connection.setReadTimeout(30_000);
                return connection;
            } catch (Exception error) {
                close(connection, connections);
                throw error;
            }
        }
        throw new Exception("重定向次数过多");
    }

    static boolean rateLimited(HttpURLConnection connection, String original) throws Exception {
        int status = connection.getResponseCode();
        String type = connection.getContentType();
        return status == 429 || (status == 403 && (
            isGithubAsset(new URL(original)) || connection.getHeaderField("Retry-After") != null
                || "0".equals(connection.getHeaderField("X-RateLimit-Remaining"))
                || (type != null && type.contains("json"))
        ));
    }

    static int remaining(long deadline) throws Exception {
        long remaining = deadline - SystemClock.elapsedRealtime();
        if (remaining <= 0) throw new Exception("连接下载源超时");
        return (int) Math.min(8_000, remaining);
    }

    static void close(HttpURLConnection connection, Set<HttpURLConnection> connections) {
        if (connection == null) return;
        connections.remove(connection);
        connection.disconnect();
    }

    private static int port(URL url) {
        return url.getPort() == -1 ? 443 : url.getPort();
    }
}
