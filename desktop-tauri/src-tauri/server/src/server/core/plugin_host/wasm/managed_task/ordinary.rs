//! 普通网络的固定任务边界。策略只从宿主已验证的 Options 派生，不授予任务执行权限。

use agent2api_provider_protocol as proto;
use reqwest::header::HeaderMap;
use url::Url;

use super::{policy, types::ValidationError};
use super::super::manifest::Options;

/// 字段私有且无 Default/反序列化入口；复制只用于同一次可信下载的后续跳转。
#[derive(Clone, Copy)]
pub(in crate::server::core::plugin_host::wasm) struct OrdinaryPolicy {
    guarded: bool,
    provider: &'static str,
}

impl OrdinaryPolicy {
    pub(in crate::server::core::plugin_host::wasm) fn from_options(options: &Options) -> Result<Self, proto::WireError> {
        let guarded = match &options.manifest.managed_task {
            Some(declaration) => declaration.matches_call(&options.provider, policy::ENTRY)
                .map_err(|_| denied("受控任务声明与当前宿主提供商不一致，未开放普通网络"))?,
            None => false,
        };
        let provider = if options.provider == policy::INTL_PROVIDER { policy::INTL_PROVIDER } else { policy::PROVIDER };
        Ok(Self { guarded, provider })
    }

    pub(in crate::server::core::plugin_host::wasm) fn applies(self) -> bool {
        self.guarded
    }

    pub(in crate::server::core::plugin_host::wasm) fn check_request(self, raw_url: &str, headers: &HeaderMap) -> Result<(), proto::WireError> {
        if !self.guarded { return Ok(()); }
        policy::guard_ordinary_request(self.provider, raw_url, headers).map_err(wire)
    }

    /// 公共下载不携带插件请求头，只使用宿主固定的图片请求头。
    pub(in crate::server::core::plugin_host::wasm) fn check_public_url(self, raw_url: &str) -> Result<(), proto::WireError> {
        self.check_request(raw_url, &HeaderMap::new())
    }

    pub(in crate::server::core::plugin_host::wasm) fn redirect(self, base: &Url, location: &str) -> Result<Url, proto::WireError> {
        if self.guarded {
            if location.starts_with("//") {
                // 补 scheme 但不折叠原始路径；空主机、额外斜线及编码仍由固定规则拒绝。
                self.check_public_url(&format!("{}:{location}", base.scheme()))?;
            } else {
                match Url::parse(location) {
                    Ok(_) => self.check_public_url(location)?,
                    Err(url::ParseError::RelativeUrlWithoutBase) => {
                        if base.host_str().is_some_and(|host| policy::hostname(self.provider).is_some_and(|expected| host.trim_end_matches('.').eq_ignore_ascii_case(expected))) {
                            let raw_path = location.split(['?', '#']).next().unwrap_or("");
                            policy::guard_ordinary_path(raw_path).map_err(wire)?;
                        }
                    }
                    Err(_) => return Err(denied("公共图片跳转地址无效")),
                }
            }
        }
        let target = base.join(location).map_err(|_| super::super::error("公共图片跳转地址无效"))?;
        // 相对路径或查询串的最终目标也必须核对，不能只检查 Location 字面量。
        self.check_public_url(target.as_str())?;
        Ok(target)
    }
}

fn denied(message: &'static str) -> proto::WireError {
    proto::WireError::with_code(message, "forbidden")
}

fn wire(error: ValidationError) -> proto::WireError {
    denied(error.message())
}
