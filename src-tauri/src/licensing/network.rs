use super::*;
use grantmint_offline::{
    ActivationChallenge, ActivationResponse, DeviceChallenge, SyncClock, VerificationState,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::io::Read;

struct Exchange {
    config: Config,
    keys: TrustedKeys,
    identity: Identity,
    previous: Option<OfflineBundle>,
    now: i64,
    started: Instant,
}

impl Exchange {
    fn expected<'a>(&'a self, license_id: &'a str) -> ExpectedDevice<'a> {
        ExpectedDevice {
            product_id: &self.config.product_id,
            license_id,
            fingerprint_hash: &self.identity.fingerprint.hash,
            fingerprint_version: 1,
        }
    }

    fn current_time(&self) -> i64 {
        utc_now().max(
            self.now
                .saturating_add(self.started.elapsed().as_secs() as i64),
        )
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T, String> {
        static CRYPTO: OnceLock<()> = OnceLock::new();
        CRYPTO.get_or_init(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "无法初始化授权网络客户端")?;
        let response = client
            .post(format!(
                "{}{}",
                self.config.server_url.trim_end_matches('/'),
                path
            ))
            .json(body)
            .send()
            .map_err(|_| "无法连接授权服务器，已有离线授权保持不变")?;
        if !response.status().is_success() {
            return Err(format!(
                "授权服务器拒绝请求（HTTP {}），请核对授权或联系管理员",
                response.status().as_u16()
            ));
        }
        if response.content_length().is_some_and(|size| size > 131072) {
            return Err("授权响应过大".into());
        }
        let mut bytes = Vec::new();
        response
            .take(131073)
            .read_to_end(&mut bytes)
            .map_err(|_| "授权响应读取失败")?;
        if bytes.len() > 131072 {
            return Err("授权响应过大".into());
        }
        let value = grantmint_offline::parse_strict_json(&bytes).map_err(|_| "授权响应格式无效")?;
        serde_json::from_value(value).map_err(|_| "授权响应字段无效".into())
    }

    fn activate(&self, license_id: &str, activation_secret: &str) -> Result<OfflineBundle, String> {
        if self.previous.is_some() {
            return Err("本机已有授权，请使用更新授权".into());
        }
        if uuid::Uuid::parse_str(license_id)
            .map(|id| id.to_string())
            .ok()
            .as_deref()
            != Some(license_id)
            || activation_secret.len() != 64
            || !activation_secret
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("请输入有效授权 UUID 和 64 位激活秘密".into());
        }
        let identity = DeviceIdentity::from_seed(self.identity.seed);
        let challenge: ActivationChallenge = self.post("/v2/licenses/challenges", &json!({"productId":self.config.product_id,"licenseId":license_id,"activationSecret":activation_secret,"devicePublicKey":identity.public_key_base64(),"fingerprintHash":self.identity.fingerprint.hash,"fingerprintVersion":1}))?;
        let signature = grantmint_offline::sign_activation_challenge(
            &challenge,
            &self.expected(license_id),
            &identity,
            self.current_time(),
        )
        .map_err(|_| "激活挑战与本机身份不一致或已过期")?;
        let response: ActivationResponse = self.post("/v2/licenses/activate", &json!({"productId":self.config.product_id,"licenseId":license_id,"challengeId":challenge.challenge_id,"signature":signature,"idempotencyKey":uuid::Uuid::new_v4().to_string()}))?;
        let verification = grantmint_offline::accept_activation(
            &response,
            &challenge,
            &self.keys,
            &self.expected(license_id),
            &identity,
            self.current_time(),
        )
        .map_err(|error| format!("激活签名验证失败：{error}"))?;
        Ok(OfflineBundle {
            token: response.token,
            state: verification.state,
        })
    }

    fn sync(&self) -> Result<OfflineBundle, String> {
        let bundle = self.previous.as_ref().ok_or("尚未激活，请先激活授权")?;
        let identity = DeviceIdentity::from_seed(self.identity.seed);
        let expected = self.expected(&bundle.state.license_id);
        let challenge: DeviceChallenge = self.post("/v2/licenses/sync-challenges", &json!({"productId":self.config.product_id,"licenseId":bundle.state.license_id,"devicePublicKey":identity.public_key_base64(),"fingerprintHash":self.identity.fingerprint.hash,"fingerprintVersion":1,"operation":"sync"}))?;
        let signature = grantmint_offline::sign_sync_challenge(
            &challenge,
            &expected,
            &identity,
            self.current_time(),
        )
        .map_err(|_| "更新授权挑战与本机不一致或已过期")?;
        let response: ActivationResponse = self.post("/v2/licenses/sync", &json!({"productId":self.config.product_id,"licenseId":bundle.state.license_id,"challengeId":challenge.challenge_id,"signature":signature,"idempotencyKey":uuid::Uuid::new_v4().to_string()}))?;
        let mut state = bundle.state.clone();
        state.max_seen_time = state.max_seen_time.max(self.now);
        let verified = grantmint_offline::accept_sync(
            &response,
            &challenge,
            &self.keys,
            &expected,
            &identity,
            &state,
            SyncClock {
                wall_now: self.current_time(),
                monotonic_elapsed_seconds: self.started.elapsed().as_secs(),
            },
        )
        .map_err(|error| format!("续期验证失败，保留原证书：{error}"))?;
        Ok(OfflineBundle {
            token: response.token,
            state: verified.state,
        })
    }

    fn acknowledge(&self) -> Result<(), String> {
        let bundle = self.previous.as_ref().ok_or("授权尚未保存")?;
        let identity = DeviceIdentity::from_seed(self.identity.seed);
        let expected = self.expected(&bundle.state.license_id);
        let challenge: DeviceChallenge = self.post("/v2/licenses/sync-challenges", &json!({"productId":self.config.product_id,"licenseId":bundle.state.license_id,"devicePublicKey":identity.public_key_base64(),"fingerprintHash":self.identity.fingerprint.hash,"fingerprintVersion":1,"operation":"ack","serial":bundle.state.serial}))?;
        let signature = grantmint_offline::sign_acknowledgement(
            &challenge,
            &expected,
            &identity,
            &bundle.state,
            self.current_time(),
        )
        .map_err(|_| "确认挑战与已保存证书不一致")?;
        let response: Value = self.post("/v2/licenses/acknowledgements", &json!({"productId":self.config.product_id,"licenseId":bundle.state.license_id,"challengeId":challenge.challenge_id,"signature":signature,"idempotencyKey":uuid::Uuid::new_v4().to_string(),"serial":bundle.state.serial}))?;
        if response["serial"].as_str() != Some(&bundle.state.serial) {
            return Err("服务器确认序号不一致".into());
        }
        Ok(())
    }
}

fn same_version(a: Option<&OfflineBundle>, b: Option<&OfflineBundle>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.state.license_id == b.state.license_id
                && a.state.revision == b.state.revision
                && a.state.binding_version == b.state.binding_version
                && a.state.serial == b.state.serial
        }
        _ => false,
    }
}

impl LicenseManager {
    fn exchange(&self, activation: bool) -> Result<Exchange, String> {
        self.check_storage_identity()?;
        self.check_storage_certificate()?;
        self.check_fingerprint()?;
        if self.recovery.is_some() {
            return Err("本地状态损坏，不能自动重建可信版本；请联系维护人员".into());
        }
        if activation && (self.saved.bundle.is_some() || self.anchor.accepted.is_some()) {
            return Err("本机已有授权，请使用更新授权；不能覆盖既有绑定".into());
        }
        if let Some(bundle) = self.saved.bundle.as_ref() {
            self.check_highwater(&bundle.state)?;
        }
        Ok(Exchange {
            config: self.config.clone(),
            keys: self.keys.clone(),
            identity: self.identity.clone(),
            previous: self.saved.bundle.clone(),
            now: self.clock(utc_now())?,
            started: Instant::now(),
        })
    }

    fn install(&mut self, exchange: &Exchange, bundle: OfflineBundle) -> Result<Exchange, String> {
        if !same_version(exchange.previous.as_ref(), self.saved.bundle.as_ref()) {
            return Err("授权已被另一操作更新，请重试".into());
        }
        self.check_storage_identity()?;
        self.check_storage_certificate()?;
        if self.recovery.is_some() {
            return Err("本地状态已标记损坏，取消联网结果保存".into());
        }
        self.check_fingerprint()?;
        self.check_highwater(&bundle.state)?;
        let now = self.clock(utc_now())?;
        let state = VerificationState {
            max_seen_time: bundle.state.max_seen_time.max(now),
            ..bundle.state.clone()
        };
        let verified = grantmint_offline::verify(
            &bundle.token,
            &self.keys,
            &self.expected(&state.license_id),
            &DeviceIdentity::from_seed(self.identity.seed),
            &state,
            now,
            0,
        )
        .map_err(|error| format!("落盘前授权验证失败：{error}"))?;
        if !verified.status.allows_new_work() {
            return Err("授权在保存前已到期，保留原状态".into());
        }
        self.persist_bundle(OfflineBundle {
            token: bundle.token,
            state: verified.state,
        })?;
        self.exchange(false)
    }

    fn persist_bundle(&mut self, bundle: OfflineBundle) -> Result<(), String> {
        self.check_highwater(&bundle.state)?;
        let mut anchor = self.anchor.clone();
        anchor.max_time = anchor.max_time.max(bundle.state.max_seen_time);
        anchor.accepted = Some(bundle.state.clone());
        let next = SavedState {
            bundle: Some(bundle),
        };
        self.store.write("state.bin", &next)?;
        if let Err(error) = self.persist_anchor(anchor) {
            if self.store.write("state.bin", &self.saved).is_err() {
                self.recovery = Some("证书事务回滚失败，请联系维护人员".into());
            }
            return Err(error);
        }
        self.saved = next;
        Ok(())
    }

    #[cfg(test)]
    pub fn install_test_fixture(&mut self, bundle: OfflineBundle) -> Result<(), String> {
        self.persist_bundle(bundle)
    }

    #[cfg(test)]
    pub fn activate(&mut self, license_id: &str, secret: &str) -> Result<LicenseStatus, String> {
        let exchange = self.exchange(true)?;
        let bundle = exchange.activate(license_id, secret)?;
        let ack = self.install(&exchange, bundle)?.acknowledge();
        Ok(delivery_status(self.status(), ack.is_ok()))
    }
    #[cfg(test)]
    pub fn sync(&mut self) -> Result<LicenseStatus, String> {
        let exchange = self.exchange(false)?;
        let bundle = exchange.sync()?;
        let ack = self.install(&exchange, bundle)?.acknowledge();
        Ok(delivery_status(self.status(), ack.is_ok()))
    }
}

fn delivery_status(mut status: LicenseStatus, confirmed: bool) -> LicenseStatus {
    if !confirmed {
        status
            .message
            .push_str("；证书已安全保存，但服务器尚未确认领取，下次更新授权时将重试确认");
    }
    status
}
static NETWORK: Mutex<()> = Mutex::new(());

pub async fn license_activate(
    license_id: String,
    activation_secret: String,
) -> Result<LicenseStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let secret = Zeroizing::new(activation_secret);
        let _network = NETWORK.try_lock().map_err(|_| "已有授权联网操作正在进行")?;
        let exchange = with_manager(|manager| manager.exchange(true))?;
        let bundle = exchange.activate(license_id.trim(), secret.trim())?;
        let ack_exchange = with_manager(|manager| manager.install(&exchange, bundle))?;
        let ack = ack_exchange.acknowledge();
        with_manager(|manager| Ok(delivery_status(manager.status(), ack.is_ok())))
    })
    .await
    .map_err(|_| "激活任务失败".to_string())?
}
pub async fn license_sync() -> Result<LicenseStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let _network = NETWORK.try_lock().map_err(|_| "已有授权联网操作正在进行")?;
        let exchange = with_manager(|manager| manager.exchange(false))?;
        let bundle = exchange.sync()?;
        let ack_exchange = with_manager(|manager| manager.install(&exchange, bundle))?;
        let ack = ack_exchange.acknowledge();
        with_manager(|manager| Ok(delivery_status(manager.status(), ack.is_ok())))
    })
    .await
    .map_err(|_| "更新授权任务失败".to_string())?
}
