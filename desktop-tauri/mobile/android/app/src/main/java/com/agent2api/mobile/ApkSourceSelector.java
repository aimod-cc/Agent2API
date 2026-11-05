package com.agent2api.mobile;

import android.content.Context;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.SystemClock;
import com.getcapacitor.JSArray;
import com.getcapacitor.JSObject;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorCompletionService;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

final class ApkSourceSelector {
    private static final long TTL_MS = 10 * 60_000;
    private static final long TIMEOUT_MS = 6_000;
    private static final int SAMPLE_BYTES = 64 * 1024;
    private final Map<String, Choice> choices = new HashMap<>();
    private final Set<HttpURLConnection> connections = ConcurrentHashMap.newKeySet();

    private static final class Choice {
        final String url;
        final long until;
        final long elapsed;
        Choice(String url, long until, long elapsed) {
            this.url = url;
            this.until = until;
            this.elapsed = elapsed;
        }
    }

    private static final class Sample {
        final String url;
        final long elapsed;
        final String error;
        Sample(String url, long elapsed, String error) {
            this.url = url;
            this.elapsed = elapsed;
            this.error = error;
        }
    }

    JSObject select(Context context, List<String> urls, String hash, String scope, AtomicBoolean cancelled) throws Exception {
        long now = SystemClock.elapsedRealtime();
        List<String> sorted = new ArrayList<>(urls);
        Collections.sort(sorted);
        String network = networkIdentity(context);
        String key = scope + "|" + network + "|" + sorted.toString();
        synchronized (choices) {
            choices.entrySet().removeIf(entry -> entry.getValue().until <= now);
            Choice cached = choices.get(key);
            if (cached != null && network != null) return result(urls, cached.url, cached.elapsed, true);
        }
        ExecutorService workers = Executors.newFixedThreadPool(Math.min(4, urls.size()));
        ExecutorCompletionService<Sample> completed = new ExecutorCompletionService<>(workers);
        List<Future<Sample>> pending = new ArrayList<>();
        long deadline = now + TIMEOUT_MS;
        Sample winner = null;
        try {
            for (String url : urls) pending.add(completed.submit(() -> sample(url, hash, deadline, cancelled)));
            for (int index = 0; index < urls.size(); index++) {
                if (cancelled.get()) throw new Exception("已取消下载");
                long remaining = deadline - SystemClock.elapsedRealtime();
                if (remaining <= 0) break;
                Future<Sample> done = completed.poll(remaining, TimeUnit.MILLISECONDS);
                if (done == null) break;
                Sample sample = done.get();
                if (sample.error == null) { winner = sample; break; }
                if (sample.error.contains("限流")) throw new Exception(sample.error);
            }
        } finally {
            for (Future<Sample> future : pending) future.cancel(true);
            workers.shutdownNow();
            cancel();
        }
        if (cancelled.get()) throw new Exception("已取消下载");
        if (winner == null) return result(urls, "", 0, false);
        if (network != null) synchronized (choices) {
            if (choices.size() >= 16) choices.clear();
            choices.put(key, new Choice(winner.url, now + TTL_MS, winner.elapsed));
        }
        return result(urls, winner.url, winner.elapsed, false);
    }

    private Sample sample(String url, String hash, long deadline, AtomicBoolean cancelled) {
        long started = SystemClock.elapsedRealtime();
        HttpURLConnection connection = null;
        try {
            if (cancelled.get() || Thread.currentThread().isInterrupted()) throw new Exception("已取消下载");
            connection = ApkUpdateConnection.open(url, hash, deadline, true, cancelled, connections);
            int status = connection.getResponseCode();
            if (ApkUpdateConnection.rateLimited(connection, url)) throw new Exception("下载源限流中，请稍后重试");
            if (status != 200 && status != 206) throw new Exception("服务器返回 HTTP " + status);
            String type = connection.getContentType();
            if (type != null && type.contains("text/html")) throw new Exception("下载源返回了网页而不是文件内容");
            long expected = connection.getContentLengthLong();
            if (status == 206) {
                String range = connection.getHeaderField("Content-Range");
                if (range == null || !range.matches("bytes 0-[0-9]+/([0-9]+|\\*)")) {
                    throw new Exception("下载源返回的分段内容无效");
                }
                String[] parts = range.substring(8).split("/");
                long end = Long.parseLong(parts[0]);
                if (end >= SAMPLE_BYTES || (!parts[1].equals("*") && Long.parseLong(parts[1]) <= end)) {
                    throw new Exception("下载源返回的分段内容无效");
                }
                expected = end + 1;
            }
            if (expected > SAMPLE_BYTES) expected = SAMPLE_BYTES;
            int received = 0;
            byte[] buffer = new byte[8192];
            try (InputStream input = connection.getInputStream()) {
                while (received < SAMPLE_BYTES) {
                    if (cancelled.get() || Thread.currentThread().isInterrupted()) throw new Exception("已取消下载");
                    connection.setReadTimeout(ApkUpdateConnection.remaining(deadline));
                    int read = input.read(buffer, 0, Math.min(buffer.length, SAMPLE_BYTES - received));
                    if (read == -1) break;
                    if (received == 0) {
                        String prefix = new String(buffer, 0, Math.min(read, 256), java.nio.charset.StandardCharsets.UTF_8)
                            .trim().toLowerCase(java.util.Locale.ROOT);
                        if (prefix.startsWith("<!doctype html") || prefix.startsWith("<html")) {
                            throw new Exception("下载源返回了网页而不是文件内容");
                        }
                    }
                    received += read;
                }
            }
            if (received == 0 || (expected >= 0 && received != expected)) throw new Exception("下载源返回的文件内容不完整");
            return new Sample(url, SystemClock.elapsedRealtime() - started, null);
        } catch (Exception error) {
            return new Sample(url, SystemClock.elapsedRealtime() - started, error.getMessage() == null ? "探测失败" : error.getMessage());
        } finally {
            ApkUpdateConnection.close(connection, connections);
        }
    }

    void forget(String url) {
        synchronized (choices) { choices.entrySet().removeIf(entry -> entry.getValue().url.equals(url)); }
    }

    void cancel() {
        for (HttpURLConnection connection : connections) ApkUpdateConnection.close(connection, connections);
    }

    private static JSObject result(List<String> urls, String winner, long elapsed, boolean cached) {
        JSArray ordered = new JSArray();
        if (!winner.isEmpty()) ordered.put(winner);
        for (String url : urls) if (!url.equals(winner)) ordered.put(url);
        JSObject result = new JSObject();
        result.put("urls", ordered);
        result.put("selected", winner);
        result.put("elapsedMs", elapsed);
        result.put("cached", cached);
        return result;
    }

    private static String networkIdentity(Context context) {
        try {
            ConnectivityManager manager = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
            Network network = manager == null ? null : manager.getActiveNetwork();
            NetworkCapabilities capabilities = network == null ? null : manager.getNetworkCapabilities(network);
            return network == null ? null : network.toString() + ":" + String.valueOf(capabilities);
        } catch (SecurityException ignored) { return null; }
    }
}
