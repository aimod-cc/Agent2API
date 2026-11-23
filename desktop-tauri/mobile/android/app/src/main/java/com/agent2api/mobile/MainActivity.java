package com.agent2api.mobile;

import android.os.Bundle;
import com.getcapacitor.BridgeActivity;

public class MainActivity extends BridgeActivity {
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        registerPlugin(ApkUpdaterPlugin.class);
        registerPlugin(ImageSaverPlugin.class);
        registerPlugin(ExternalUrlPlugin.class);
        super.onCreate(savedInstanceState);
    }
}
