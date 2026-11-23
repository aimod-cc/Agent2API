package com.agent2api.mobile;

import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.net.Uri;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.annotation.CapacitorPlugin;

@CapacitorPlugin(name = "ExternalUrl")
public class ExternalUrlPlugin extends Plugin {
    @PluginMethod
    public void open(PluginCall call) {
        String raw = call.getString("url");
        if (raw == null || raw.trim().isEmpty()) {
            call.reject("Missing URL");
            return;
        }
        Uri uri = Uri.parse(raw.trim());
        String scheme = uri.getScheme();
        if (scheme == null || (!"http".equalsIgnoreCase(scheme) && !"https".equalsIgnoreCase(scheme))) {
            call.reject("Invalid external URL");
            return;
        }
        Intent intent = new Intent(Intent.ACTION_VIEW, uri);
        intent.addCategory(Intent.CATEGORY_BROWSABLE);
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        try {
            if (getActivity() != null) {
                getActivity().startActivity(intent);
            } else {
                getContext().startActivity(intent);
            }
            call.resolve();
        } catch (ActivityNotFoundException | SecurityException err) {
            call.reject("Failed to open external URL", err);
        }
    }
}
