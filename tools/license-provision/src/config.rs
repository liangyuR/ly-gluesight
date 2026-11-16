use argon2::password_hash::PasswordHash;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;

pub fn validate(value: &Value, allow_development: bool) -> Result<(), &'static str> {
    let fields = value.as_object().ok_or("授权配置必须为对象")?;
    if fields.keys().any(|key| {
        !matches!(
            key.as_str(),
            "serverUrl" | "productId" | "trustedKeys" | "emergencyPasswordHash"
        )
    }) {
        return Err("授权配置含不支持的字段；禁止写入明文密码或其他秘密");
    }
    let server = value["serverUrl"]
        .as_str()
        .ok_or("serverUrl 必须为字符串")?;
    let product = value["productId"]
        .as_str()
        .ok_or("productId 必须为字符串")?;
    let keys = value["trustedKeys"]
        .as_object()
        .ok_or("trustedKeys 必须为对象")?;
    let hash = value["emergencyPasswordHash"]
        .as_str()
        .ok_or("emergencyPasswordHash 必须为字符串")?;
    if allow_development
        && server.is_empty()
        && product.is_empty()
        && keys.is_empty()
        && hash.is_empty()
    {
        return Ok(());
    }
    let url = url::Url::parse(server).map_err(|_| "serverUrl 不是有效服务地址")?;
    let local_http = allow_development
        && url.scheme() == "http"
        && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if (url.scheme() != "https" && !local_http)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("服务地址必须为无凭据、查询或片段的 HTTPS 地址；开发模式仅额外允许本机 HTTP");
    }
    let parsed = uuid::Uuid::parse_str(product).map_err(|_| "productId 必须为有效应用 UUID")?;
    if parsed.is_nil() || parsed.is_max() || parsed.to_string() != product {
        return Err("productId 必须为非占位、小写带连字符的规范 UUID");
    }
    if keys.is_empty() || keys.len() > 64 {
        return Err("必须配置 1 至 64 个签名公钥");
    }
    for (id, key) in keys {
        let key = key.as_str().ok_or("公钥必须为 Base64 字符串")?;
        let bytes = STANDARD
            .decode(key)
            .map_err(|_| "签名公钥不是有效 Base64")?;
        if id.is_empty()
            || id.len() > 128
            || id.trim() != id
            || id.chars().any(char::is_control)
            || bytes.len() != 32
            || bytes.iter().all(|byte| *byte == 0)
            || STANDARD.encode(&bytes) != key
        {
            return Err("公钥必须使用非空 key ID 和规范 Base64 编码的 32 字节公钥");
        }
    }
    if !hash.is_empty() && !valid_password_hash(hash) {
        return Err("应急校验值必须为参数受限、含盐及摘要的 Argon2id PHC，不能填写明文密码");
    }
    Ok(())
}

fn valid_password_hash(value: &str) -> bool {
    if value.len() > 512 {
        return false;
    }
    let Ok(hash) = PasswordHash::new(value) else {
        return false;
    };
    let mut salt = [0; 64];
    hash.algorithm.as_str() == "argon2id"
        && hash.version == Some(19)
        && hash.params.iter().count() == 3
        && hash
            .params
            .get_decimal("m")
            .is_some_and(|m| (8192..=262144).contains(&m))
        && hash
            .params
            .get_decimal("t")
            .is_some_and(|t| (1..=10).contains(&t))
        && hash
            .params
            .get_decimal("p")
            .is_some_and(|p| (1..=8).contains(&p))
        && hash.salt.is_some_and(|value| {
            value
                .decode_b64(&mut salt)
                .is_ok_and(|bytes| (16..=64).contains(&bytes.len()))
        })
        && hash
            .hash
            .is_some_and(|output| (32..=64).contains(&output.len()))
}
