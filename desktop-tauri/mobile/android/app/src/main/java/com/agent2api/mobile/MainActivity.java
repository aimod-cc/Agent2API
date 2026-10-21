package com.agent2api.mobile;

import android.os.Bundle;
import com.getcapacitor.BridgeActivity;

public class MainActivity extends BridgeActivity {

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // 自定义插件必须在 super.onCreate 之前注册：BridgeActivity 在那里
        // 就把插件表建好了（见 BridgeActivity.load()）。
        registerPlugin(ApkUpdaterPlugin.class);
        super.onCreate(savedInstanceState);
    }
}
