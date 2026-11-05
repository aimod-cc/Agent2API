use serde::{Deserialize, Serialize};

pub(super) const SOURCE_STATS_VERSION: i64 = 2;

/// 只保存已通过鉴权的身份，不包含密钥值；空来源表示历史未记录。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ApiKeyIdentity {
    #[serde(rename = "apiKeyId", default)]
    pub id: String,
    #[serde(rename = "apiKeyName", default)]
    pub name: String,
    #[serde(rename = "apiKeySource", default)]
    pub source: String,
}

impl ApiKeyIdentity {
    pub fn filter_value(&self) -> String {
        match self.source.as_str() {
            "key" => format!("key:{}", self.id),
            "env" | "none" => self.source.clone(),
            _ => "unknown".to_string(),
        }
    }

    pub fn from_filter(value: &str) -> Option<Self> {
        let value = value.trim();
        let (source, id) = match value {
            "env" | "none" => (value, ""),
            "unknown" => ("", ""),
            _ => ("key", value.strip_prefix("key:").filter(|id| !id.is_empty())?),
        };
        Some(Self { id: id.to_string(), name: String::new(), source: source.to_string() })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RequestSource {
    #[serde(default)]
    pub client: String,
    #[serde(flatten)]
    pub api_key: ApiKeyIdentity,
    #[serde(rename = "clientEvidence", default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<crate::server::core::client::ClientEvidence>,
    /// 请求行为只使用原来的严格识别，显示规则不能改变模型选择。
    #[serde(skip)]
    pub behavior_client: String,
}

impl RequestSource {
    pub fn is_internal(&self) -> bool {
        self.evidence.is_none() && matches!(self.client.as_str(), "model_test" | "assistant" | "assistant_images" | "assistant_title" | "checkin_keepalive")
    }

    pub fn evidence_json(&self) -> String {
        self.evidence.as_ref().map(|evidence| evidence.encoded()).unwrap_or_else(|| "{}".into())
    }

    pub fn recorded_client_name(&self) -> &str {
        self.evidence.as_ref().map(|evidence| evidence.client_name.as_str()).unwrap_or("")
    }

    pub(super) fn normalized(mut self) -> Self {
        self.client = self.client.trim().to_string();
        self.evidence = self.evidence.map(|evidence| evidence.normalized());
        self.api_key.id = self.api_key.id.trim().to_string();
        self.api_key.name = self.api_key.name.trim().to_string();
        self
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct UsageAccum {
    #[serde(default)]
    pub requests: i64,
    #[serde(default)]
    pub successful: i64,
    #[serde(default)]
    pub tokens: i64,
    #[serde(rename = "promptTokens", default)]
    pub prompt_tokens: i64,
    #[serde(rename = "completionTokens", default)]
    pub completion_tokens: i64,
    #[serde(rename = "cacheReadTokens", default)]
    pub cache_read_tokens: i64,
    /// 缺少明细的历史余量只有总 Token，不能据此编造输入、输出或缓存读数。
    #[serde(rename = "tokenDetailsRequests", default)]
    pub token_details_requests: i64,
    #[serde(rename = "lastUsedAt", default)]
    pub last_used_at: Option<i64>,
}

impl UsageAccum {
    pub(super) fn merge(&mut self, other: &Self) {
        self.requests += other.requests;
        self.successful += other.successful;
        self.tokens += other.tokens;
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.token_details_requests += other.token_details_requests;
        self.last_used_at = self.last_used_at.max(other.last_used_at);
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct ClientAccum {
    #[serde(default)]
    pub client: String,
    #[serde(default)]
    pub label: String,
    #[serde(flatten)]
    pub usage: UsageAccum,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct ApiKeyAccum {
    #[serde(flatten)]
    pub identity: ApiKeyIdentity,
    #[serde(flatten)]
    pub usage: UsageAccum,
    #[serde(default)]
    pub clients: Vec<ClientAccum>,
    #[serde(default)]
    pub models: Vec<super::record::ModelAccum>,
}
