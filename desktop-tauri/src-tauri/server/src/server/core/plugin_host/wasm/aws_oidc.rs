//! AWS 商业区 OIDC 精确来源；认证目录地区与 Kiro 订阅业务地区分别处理。
//! 来源：https://docs.aws.amazon.com/general/latest/gr/sso.html 及 AWS botocore endpoints.json。

pub const REGIONS: &[&str] = &[
    "us-east-1", "eu-central-1", "af-south-1", "ap-east-1", "ap-east-2",
    "ap-northeast-1", "ap-northeast-2", "ap-northeast-3", "ap-south-1", "ap-south-2",
    "ap-southeast-1", "ap-southeast-2", "ap-southeast-3", "ap-southeast-4", "ap-southeast-5",
    "ap-southeast-6", "ap-southeast-7", "ca-central-1", "ca-west-1", "eu-central-2",
    "eu-north-1", "eu-south-1", "eu-south-2", "eu-west-1", "eu-west-2", "eu-west-3",
    "il-central-1", "me-central-1", "me-south-1", "mx-central-1", "sa-east-1",
    "us-east-2", "us-west-1", "us-west-2",
];

pub const ORIGINS: &[&str] = &[
    "https://oidc.us-east-1.amazonaws.com",
    "https://oidc.eu-central-1.amazonaws.com",
    "https://oidc.af-south-1.amazonaws.com",
    "https://oidc.ap-east-1.amazonaws.com",
    "https://oidc.ap-east-2.amazonaws.com",
    "https://oidc.ap-northeast-1.amazonaws.com",
    "https://oidc.ap-northeast-2.amazonaws.com",
    "https://oidc.ap-northeast-3.amazonaws.com",
    "https://oidc.ap-south-1.amazonaws.com",
    "https://oidc.ap-south-2.amazonaws.com",
    "https://oidc.ap-southeast-1.amazonaws.com",
    "https://oidc.ap-southeast-2.amazonaws.com",
    "https://oidc.ap-southeast-3.amazonaws.com",
    "https://oidc.ap-southeast-4.amazonaws.com",
    "https://oidc.ap-southeast-5.amazonaws.com",
    "https://oidc.ap-southeast-6.amazonaws.com",
    "https://oidc.ap-southeast-7.amazonaws.com",
    "https://oidc.ca-central-1.amazonaws.com",
    "https://oidc.ca-west-1.amazonaws.com",
    "https://oidc.eu-central-2.amazonaws.com",
    "https://oidc.eu-north-1.amazonaws.com",
    "https://oidc.eu-south-1.amazonaws.com",
    "https://oidc.eu-south-2.amazonaws.com",
    "https://oidc.eu-west-1.amazonaws.com",
    "https://oidc.eu-west-2.amazonaws.com",
    "https://oidc.eu-west-3.amazonaws.com",
    "https://oidc.il-central-1.amazonaws.com",
    "https://oidc.me-central-1.amazonaws.com",
    "https://oidc.me-south-1.amazonaws.com",
    "https://oidc.mx-central-1.amazonaws.com",
    "https://oidc.sa-east-1.amazonaws.com",
    "https://oidc.us-east-2.amazonaws.com",
    "https://oidc.us-west-1.amazonaws.com",
    "https://oidc.us-west-2.amazonaws.com",
];

pub fn origin(region: &str) -> Option<&'static str> {
    REGIONS.iter().position(|value| *value == region).and_then(|index| ORIGINS.get(index).copied())
}

pub fn token_region(endpoint: &str) -> Option<&'static str> {
    let origin = endpoint.strip_suffix("/token")?;
    ORIGINS.iter().position(|value| *value == origin).and_then(|index| REGIONS.get(index).copied())
}
