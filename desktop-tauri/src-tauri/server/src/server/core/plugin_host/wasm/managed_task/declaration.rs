//! 声明只能选择宿主固定 profile；它不是用户批准或任务执行凭证。

use serde::{
    de::{DeserializeOwned, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use sha2::{Digest, Sha256};

use super::{
    policy,
    strict_json::{self, Object},
    types,
};

const INVALID: &str =
    "受控任务声明无效：只允许至多一个与 MonkeyCode 地区一致的固定配置，不接受额外配置";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Declaration {
    profiles: Vec<ProfileRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProfileRequest {
    #[serde(deserialize_with = "fixed_profile")]
    profile: String,
}

fn fixed_profile<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let profile = String::deserialize(deserializer)?;
    if ![policy::PROFILE, policy::INTL_PROFILE].contains(&profile.as_str()) {
        return Err(serde::de::Error::custom(INVALID));
    }
    Ok(profile)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDeclaration {
    #[serde(deserialize_with = "profiles")]
    profiles: Vec<Object<ProfileRequest>>,
}

fn profiles<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Object<ProfileRequest>>, D::Error> {
    struct Profiles;
    impl<'de> Visitor<'de> for Profiles {
        type Value = Vec<Object<ProfileRequest>>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("at most one fixed managed task profile")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
            let mut profiles = Vec::new();
            while let Some(profile) = sequence.next_element::<Object<ProfileRequest>>()? {
                if profiles.len() >= types::MAX_PROFILES {
                    return Err(serde::de::Error::custom(INVALID));
                }
                profiles.push(profile);
            }
            Ok(profiles)
        }
    }
    deserializer.deserialize_seq(Profiles)
}

impl<'de> Deserialize<'de> for Declaration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Object::<RawDeclaration>::deserialize(deserializer)?.0;
        let declaration = Self {
            profiles: raw.profiles.into_iter().map(|item| item.0).collect(),
        };
        declaration.validate().map_err(serde::de::Error::custom)?;
        Ok(declaration)
    }
}

/// 可省略声明，但显式 null、数组形状和未知字段不是有效声明。
pub(crate) fn optional<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Declaration>, D::Error> {
    Declaration::deserialize(deserializer).map(Some)
}

pub(crate) use super::strict_json::StrictJsonError as ParseObjectError;

/// 必须在转成 Value 前检查重复键，且顶层只能是对象。
pub(crate) fn parse_object<T: DeserializeOwned>(
    bytes: &[u8],
    limit: usize,
) -> Result<T, &'static str> {
    strict_json::validate(bytes, limit).map_err(|_| "JSON 格式无效或超过允许范围")?;
    serde_json::from_slice::<Object<T>>(bytes)
        .map(|object| object.0)
        .map_err(|_| "JSON 对象字段无效")
}

/// 与 `parse_object` 校验规则一致，额外区分超体积、嵌套过深、重复字段与普通格式错误，
/// 并允许调用方按场景指定最大容器嵌套层数（受 serde_json 默认 128 层上限约束）。
pub(crate) fn parse_object_detailed<T: DeserializeOwned>(
    bytes: &[u8],
    limit: usize,
    max_depth: usize,
) -> Result<T, ParseObjectError> {
    strict_json::validate_detailed_with_depth(bytes, limit, max_depth)?;
    serde_json::from_slice::<Object<T>>(bytes)
        .map(|object| object.0)
        .map_err(|error| {
            if error.is_syntax() && error.to_string().contains("recursion limit exceeded") {
                ParseObjectError::DepthExceeded
            } else {
                ParseObjectError::Invalid
            }
        })
}

impl Declaration {
    fn validate(&self) -> Result<(), &'static str> {
        if self.profiles.len() > types::MAX_PROFILES
            || self
                .profiles
                .iter()
                .any(|item| ![policy::PROFILE, policy::INTL_PROFILE].contains(&item.profile.as_str()))
        {
            return Err(INVALID);
        }
        let encoded = serde_json::to_vec(self).map_err(|_| INVALID)?;
        if encoded.len() > types::MAX_DECLARATION_BYTES {
            return Err(INVALID);
        }
        Ok(())
    }

    pub(crate) fn has_profile(&self) -> bool { self.requested() }

    pub(crate) fn validate_provider(&self, provider: &str) -> Result<(), String> {
        self.matches_call(provider, policy::ENTRY).map(|_| ())
    }

    /// 只校验声明与宿主提供的真实入口一致，不授予创建或清理权限。
    pub(super) fn matches_call(&self, provider: &str, entry: &str) -> Result<bool, String> {
        self.validate().map_err(str::to_string)?;
        if self.requested() && (policy::profile(provider) != self.profiles.first().map(|item| item.profile.as_str()) || entry != policy::ENTRY) {
            return Err("受控任务声明与当前地区的提供商或对话入口不一致".into());
        }
        Ok(self.requested())
    }

    pub(super) fn value_shape(value: &serde_json::Value) -> bool {
        let Some(fields) = value.as_object() else {
            return false;
        };
        let Some(profiles) = fields.get("profiles").and_then(serde_json::Value::as_array) else {
            return false;
        };
        fields.len() == 1
            && profiles.len() <= types::MAX_PROFILES
            && profiles.iter().all(|item| {
                item.as_object().is_some_and(|fields| {
                    fields.len() == 1
                        && fields.get("profile").and_then(serde_json::Value::as_str)
                            .is_some_and(|profile| [policy::PROFILE, policy::INTL_PROFILE].contains(&profile))
                })
            })
    }

    pub(super) fn requested(&self) -> bool {
        !self.profiles.is_empty()
    }

    pub(super) fn digest(&self) -> Result<String, &'static str> {
        self.validate()?;
        // 固定规则与声明一起绑定；宿主改动地址或预算后，旧摘要不能继续匹配。
        let profiles: Vec<_> = self.profiles.iter().map(|item| rules_for_profile(&item.profile)).collect();
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schema": "agent2api.managed-task.declaration.v1",
            "declaration": self,
            "profiles": profiles,
            "limits": {
                "prepareMs": types::PREPARE_TIMEOUT.as_millis(),
                "createMs": types::CREATE_TIMEOUT.as_millis(),
                "stopMs": types::STOP_TIMEOUT.as_millis(),
                "lifetimeMs": types::MAX_LIFETIME.as_millis(),
                "perAccount": types::UNSETTLED_PER_ACCOUNT,
                "perPackage": types::UNSETTLED_PER_PACKAGE,
                "global": types::UNSETTLED_GLOBAL,
                "createBytes": types::MAX_CREATE_BYTES,
                "creationResponseBytes": types::MAX_CREATION_RESPONSE_BYTES,
                "stopResponseBytes": types::MAX_STOP_RESPONSE_BYTES
            }
        }))
        .map_err(|_| INVALID)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Serialize)]
pub(super) struct Route {
    method: &'static str,
    path: &'static str,
}

/// 仅可由宿主构造，不从清单、旧授权记录或管理请求反序列化地址与预算。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Rules {
    pub(super) provider: &'static str,
    pub(super) profile: &'static str,
    pub(super) entry: &'static str,
    origin: &'static str,
    create: Route,
    stop: Route,
    max_creates_per_call: usize,
    max_stops_per_creation: usize,
    stopping_is_best_effort: bool,
    remote_tools_auto_approved: bool,
    questions_return_to_agent: bool,
}

pub(super) fn rules_for(provider: &str) -> Rules {
    let intl = provider == policy::INTL_PROVIDER;
    Rules {
        provider: if intl { policy::INTL_PROVIDER } else { policy::PROVIDER },
        profile: if intl { policy::INTL_PROFILE } else { policy::PROFILE },
        entry: policy::ENTRY,
        origin: if intl { "https://monkeycode-ai.net" } else { policy::ORIGIN },
        create: Route {
            method: "POST",
            path: policy::TASKS_PATH,
        },
        stop: Route {
            method: "PUT",
            path: policy::STOP_PATH,
        },
        max_creates_per_call: types::CREATE_SUBMISSIONS_PER_CALL,
        max_stops_per_creation: types::STOP_ATTEMPTS_PER_CREATION,
        stopping_is_best_effort: true,
        remote_tools_auto_approved: true,
        questions_return_to_agent: true,
    }
}

pub(super) fn rules() -> Rules { rules_for(policy::PROVIDER) }

fn rules_for_profile(profile: &str) -> Rules {
    rules_for(if profile == policy::INTL_PROFILE { policy::INTL_PROVIDER } else { policy::PROVIDER })
}

pub(super) fn pack_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(super) fn digest_text(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
