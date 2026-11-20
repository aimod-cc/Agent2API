package com.agent2api.mobile;

import android.os.SystemClock;
import java.net.HttpURLConnection;
import java.net.URL;
import java.util.Set;
import java.util.concurrent.atomic.AtomicBoolean;

final class ApkUpdateConnection {
    static boolean isWebsiteAsset(URL url) {
        if (!"https".equals(url.getProtocol())
            || (url.getPort() != -1 && url.getPort() != 443)
            || url.getUserInfo() != null || url.getQuery() != null || url.getRef() != null) {
            return false;
        }
        String host = url.getHost();
        String path = url.getPath();
        if ("agent2api.dev".equalsIgnoreCase(host)) return path.startsWith("/downloads/");
        if ("cdn.agent2api.dev".equalsIgnoreCase(host)) return path.startsWith("/agent2api/installers/");
        return false;
    }

    static HttpURLConnection open(String startUrl, long deadline,
        AtomicBoolean cancelled, Set<HttpURLConnection> connections) throws Exception {
        URL target = new URL(startUrl);
        for (int hop = 0; hop < 5; hop++) {
            if (cancelled.get() || Thread.currentThread().isInterrupted()) throw new Exception("已取消下载");
            if (!isWebsiteAsset(target)) throw new Exception("更新包只能从 Agent2API 官网下载");
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
                int status = connection.getResponseCode();
                if (status == 301 || status == 302 || status == 303 || status == 307 || status == 308) {
                    String location = connection.getHeaderField("Location");
                    if (location == null || location.isEmpty()) throw new Exception("重定向缺少 Location");
                    target = new URL(target, location);
                    close(connection, connections);
                    continue;
                }
                connection.setReadTimeout(30_000);
                return connection;
            } catch (Exception error) {
                close(connection, connections);
                throw error;
            }
        }
        throw new Exception("重定向次数过多");
    }

    static int remaining(long deadline) throws Exception {
        long remaining = deadline - SystemClock.elapsedRealtime();
        if (remaining <= 0) throw new Exception("连接官网超时");
        return (int) Math.min(8_000, remaining);
    }

    static void close(HttpURLConnection connection, Set<HttpURLConnection> connections) {
        if (connection == null) return;
        connections.remove(connection);
        connection.disconnect();
    }
}
