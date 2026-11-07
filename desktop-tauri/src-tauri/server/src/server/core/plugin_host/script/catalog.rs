use serde_json::{json, Value};

pub(super) fn recommendations() -> Value {
    json!([
        { "name": super::MINIMAX_PACKAGE, "version": super::MINIMAX_VERSION,
          "label": "MiniMax Code 国内／国际", "providers": ["minimax-code", "minimax-code-global"] },
        { "name": "magpie-deepaicode", "version": "0.2.0", "label": "DeepAI Code", "providers": ["deepaicode"] },
        { "name": "@magpie-community/opencode-cloudflare-workers-ai-auth", "version": "0.0.5",
          "source": "magpie:@magpie-community/opencode-cloudflare-workers-ai-auth",
          "label": "Cloudflare Workers AI", "providers": ["cloudflare-workers-ai"] },
        { "name": "magpie-plugin-codar", "version": "0.1.2", "label": "Codar（科搭）", "providers": ["codar"] },
        { "name": "@magpie-community/opencode-joycode-auth", "version": "0.2.7",
          "source": "magpie:@magpie-community/opencode-joycode-auth", "label": "JoyCode（京东）", "providers": ["joycode"] },
        { "name": "@magpie-community/opencode-agnes-code-auth", "version": "0.1.2",
          "source": "magpie:@magpie-community/opencode-agnes-code-auth", "label": "Agnes Code", "providers": ["agnes-code"] },
        { "name": "@magpie-community/opencode-river-auth", "version": "0.1.1",
          "source": "magpie:@magpie-community/opencode-river-auth", "label": "River", "providers": ["river"] },
        { "name": "@kevinfx/magpie-wps-lingxi", "version": "0.3.4",
          "source": "github:KevinFX-code/magpie-wps-lingxi", "selector": "",
          "label": "WPS 灵犀", "providers": ["wps-lingxi"] }
    ])
}

pub(super) fn label(id: &str) -> Option<&'static str> {
    Some(match id {
        "minimax-code" => "MiniMax Code 国内版",
        "minimax-code-global" => "MiniMax Code 国际版",
        "deepaicode" => "DeepAI Code",
        "cloudflare-workers-ai" => "Cloudflare Workers AI",
        "codar" => "Codar（科搭）",
        "joycode" => "JoyCode（京东）",
        "agnes-code" => "Agnes Code",
        "river" => "River",
        "wps-lingxi" => "WPS 灵犀",
        _ => return None,
    })
}
