#![cfg(windows)]
use super::*;
use argon2::{password_hash::SaltString, PasswordHasher};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    sync::{atomic::AtomicI64, Arc},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("gluesight-license-test-{}", uuid::Uuid::new_v4())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn config() -> Config {
    Config {
        server_url: "https://license.example.invalid".into(),
        product_id: "ca7e80ee-e477-4b35-9e5b-f195e9f3bf04".into(),
        trusted_keys: BTreeMap::from([(
            "test-key".into(),
            STANDARD.encode(SigningKey::from_bytes(&[42; 32]).verifying_key().as_bytes()),
        )]),
        emergency_password_hash: Argon2::default()
            .hash_password(
                b"test-emergency-password",
                &SaltString::encode_b64(b"test-only-salt-123").unwrap(),
            )
            .unwrap()
            .to_string(),
    }
}
fn signed_bundle(manager: &LicenseManager, now: i64, expires: i64, revision: i64) -> OfflineBundle {
    let identity = DeviceIdentity::from_seed(manager.identity.seed);
    let license_id = "ac378614-1475-456d-85c9-d6cf979d2363";
    let serial = uuid::Uuid::new_v4().to_string();
    let claims = json!({"ver":2,"mode":"offline_device","lid":license_id,"kid":"LIC-test","product_id":manager.config.product_id,"prd":"gluesight-test","serial":serial,"revision":revision,"binding_version":1,"device_key_hash":identity.key_hash(),"fingerprint_hash":manager.identity.fingerprint.hash,"fingerprint_version":1,"iat":now-60,"nbf":now-60,"exp":expires,"ent":{},"seat":1});
    let header = json!({"alg":"EdDSA","crv":"Ed25519","typ":"GMT","kid":"test-key"});
    let message = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap()),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
    );
    let token = format!(
        "{message}.{}",
        URL_SAFE_NO_PAD.encode(
            SigningKey::from_bytes(&[42; 32])
                .sign(message.as_bytes())
                .to_bytes()
        )
    );
    OfflineBundle {
        token,
        state: grantmint_offline::VerificationState {
            product_id: manager.config.product_id.clone(),
            license_id: license_id.into(),
            device_key_hash: identity.key_hash(),
            revision,
            binding_version: 1,
            max_seen_time: now,
            serial,
        },
    }
}
fn status_at(manager: &mut LicenseManager, time: i64) -> LicenseStatus {
    manager.set_test_time(time);
    manager.status()
}
fn emergency_at(
    manager: &mut LicenseManager,
    password: &str,
    reason: &str,
    time: i64,
) -> Result<LicenseStatus, String> {
    manager.set_test_time(time);
    manager.emergency_unlock(password, reason)
}
fn open_clock(cfg: Config, path: PathBuf) -> LicenseManager {
    LicenseManager::open_with_clock(cfg, path, Arc::new(AtomicI64::new(utc_now()))).unwrap()
}
fn install(manager: &mut LicenseManager, bundle: OfflineBundle) {
    manager.install_test_fixture(bundle).unwrap();
}

#[test]
fn bound_fingerprint_normalization_preserves_identity_and_missing_bound_component_fails() {
    let original = Fingerprint::from_identifiers(
        "app",
        "{01234567-89ab-cdef-0123-456789abcdef}",
        Some("abcdefab-cdef-abcd-efab-cdefabcdefab"),
        None,
    )
    .unwrap();
    let same = Fingerprint::from_identifiers(
        "app",
        "0123456789ABCDEF0123456789ABCDEF",
        Some("abcdefabcdefabcdefabcdefabcdefab"),
        Some(true),
    )
    .unwrap();
    assert_eq!(original.hash, same.hash);
    assert!(Fingerprint::from_identifiers(
        "app",
        "0123456789abcdef0123456789abcdef",
        None,
        Some(true)
    )
    .is_err());
    let changed = Fingerprint::from_identifiers(
        "app",
        "1123456789abcdef0123456789abcdef",
        Some("abcdefabcdefabcdefabcdefabcdefab"),
        Some(true),
    )
    .unwrap();
    assert_ne!(original.hash, changed.hash);
    let actual = Fingerprint::collect("app", None).unwrap();
    assert_eq!(
        actual.hash,
        Fingerprint::collect("app", Some(actual.with_firmware))
            .unwrap()
            .hash
    );
}

#[test]
fn protected_storage_prevents_plaintext_tampering_and_concurrent_writer() {
    let dir = Temp::new();
    let store = Store::open(dir.0.clone()).unwrap();
    store
        .write("state.bin", &json!({"secret":"local-device-seed-test"}))
        .unwrap();
    assert!(Store::open(dir.0.clone()).is_err());
    let mut bytes = fs::read(dir.0.join("state.bin")).unwrap();
    assert!(!bytes
        .windows(b"local-device-seed-test".len())
        .any(|window| window == b"local-device-seed-test"));
    store
        .write("state.bin", &json!({"secret":"replacement"}))
        .unwrap();
    assert_eq!(
        store.read::<serde_json::Value>("state.bin").unwrap()["secret"],
        "replacement"
    );
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(dir.0.join("state.bin"), bytes).unwrap();
    assert!(store.read::<serde_json::Value>("state.bin").is_err());
}

#[test]
fn persisted_authorization_warns_at_seven_days_and_cannot_reactivate_after_expiry_or_rollback() {
    let dir = Temp::new();
    let cfg = config();
    let now = utc_now();
    let mut manager = open_clock(cfg.clone(), dir.0.clone());
    let bundle = signed_bundle(&manager, now, now + 8 * 86400, 1);
    install(&mut manager, bundle);
    assert_eq!(status_at(&mut manager, now + 86400 - 1).state, "valid");
    assert_eq!(status_at(&mut manager, now + 86400).state, "expiring");
    assert!(status_at(&mut manager, now + 8 * 86400 - 1).allowed);
    assert_eq!(status_at(&mut manager, now + 8 * 86400).state, "expired");
    drop(manager);
    let mut manager = open_clock(cfg, dir.0.clone());
    assert!(!status_at(&mut manager, now).allowed);
    assert_eq!(status_at(&mut manager, now + 8 * 86400).state, "expired");
}

#[test]
fn restoring_only_an_old_certificate_cannot_lower_persisted_revision() {
    let dir = Temp::new();
    let cfg = config();
    let now = utc_now();
    let mut manager = open_clock(cfg.clone(), dir.0.clone());
    let first = signed_bundle(&manager, now, now + 86400, 1);
    install(&mut manager, first);
    let previous = fs::read(dir.0.join("state.bin")).unwrap();
    let renewed = signed_bundle(&manager, now, now + 86400 * 30, 2);
    install(&mut manager, renewed);
    drop(manager);
    fs::write(dir.0.join("state.bin"), previous).unwrap();
    let mut manager = open_clock(cfg, dir.0.clone());
    assert_eq!(manager.status().state, "invalid");
    assert!(manager.require_business().is_err());
}

#[test]
fn running_client_detects_deleted_or_modified_protected_files_before_new_work() {
    for file in ["state.bin", "identity.bin", "anchor.bin"] {
        for delete in [true, false] {
            let dir = Temp::new();
            let mut manager = open_clock(config(), dir.0.clone());
            let now = utc_now();
            let bundle = signed_bundle(&manager, now, now + 86400, 1);
            install(&mut manager, bundle);
            assert!(manager.require_business().is_ok());
            if delete {
                fs::remove_file(dir.0.join(file)).unwrap();
            } else {
                fs::write(dir.0.join(file), b"corrupt").unwrap();
            }
            assert!(
                manager.require_business().is_err(),
                "{file} deletion={delete}"
            );
        }
    }
}

#[test]
fn emergency_unlock_is_audited_rate_limited_and_expires_after_72_hours_across_restart() {
    let dir = Temp::new();
    let cfg = config();
    let now = utc_now();
    let mut manager = open_clock(cfg.clone(), dir.0.clone());
    assert!(emergency_at(&mut manager, "wrong", "test", now).is_err());
    assert!(emergency_at(&mut manager, "test-emergency-password", "现场恢复", now).is_err());
    let granted =
        emergency_at(&mut manager, "test-emergency-password", "现场恢复", now + 2).unwrap();
    assert_eq!(granted.state, "emergency");
    assert!(granted.allowed);
    assert!(emergency_at(&mut manager, "test-emergency-password", "不得叠加", now + 3).is_err());
    let until = now + 2 + 72 * 3600;
    drop(manager);
    let mut manager = open_clock(cfg, dir.0.clone());
    assert_eq!(
        status_at(&mut manager, until - 1).emergency_until,
        granted.emergency_until
    );
    assert!(status_at(&mut manager, until - 1).allowed);
    assert!(!status_at(&mut manager, until).allowed);
    let anchor: serde_json::Value = manager.store.read("anchor.bin").unwrap();
    assert_eq!(anchor["audit"][0]["event"], "emergency_denied");
    assert_eq!(anchor["audit"][1]["reason"], "现场恢复");
}

#[test]
fn emergency_can_handle_corrupt_certificate_but_not_a_lost_trusted_anchor() {
    let dir = Temp::new();
    let mut manager = open_clock(config(), dir.0.clone());
    fs::write(dir.0.join("state.bin"), b"damaged").unwrap();
    assert_eq!(manager.status().state, "invalid");
    assert!(
        manager
            .emergency_unlock("test-emergency-password", "修复证书文件")
            .unwrap()
            .allowed
    );
    fs::remove_file(dir.0.join("anchor.bin")).unwrap();
    assert!(!manager.status().allowed);
    assert!(manager
        .emergency_unlock("test-emergency-password", "不能重置信任锚点")
        .is_err());
}

#[test]
fn frequent_checks_do_not_discard_monotonic_time_or_extend_emergency() {
    let dir = Temp::new();
    let mut manager = open_clock(config(), dir.0.clone());
    let now = utc_now();
    emergency_at(&mut manager, "test-emergency-password", "单调时间回归", now).unwrap();
    let until = now + 72 * 3600;
    assert!(status_at(&mut manager, until - 1).allowed);
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1100) {
        let _ = status_at(&mut manager, until - 1);
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!status_at(&mut manager, until - 1).allowed);
}

#[test]
fn stalled_activation_http_does_not_hold_the_business_admission_lock() {
    use std::{io::Read, net::TcpListener, sync::mpsc};
    let dir = Temp::new();
    let mut cfg = config();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    cfg.server_url = format!("http://{}", listener.local_addr().unwrap());
    let mut manager = open_clock(cfg, dir.0.clone());
    manager
        .emergency_unlock("test-emergency-password", "网络延迟验收")
        .unwrap();
    let global = MANAGER.get_or_init(|| Mutex::new(Err("测试实例未设置".into())));
    let original = std::mem::replace(&mut *global.lock().unwrap(), Ok(manager));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut buf = [0; 4096];
        let _ = socket.read(&mut buf);
        started_tx.send(()).unwrap();
        let _ = release_rx.recv();
    });
    let request = std::thread::spawn(|| {
        tauri::async_runtime::block_on(license_activate(
            "ac378614-1475-456d-85c9-d6cf979d2363".into(),
            "a".repeat(64),
        ))
    });
    started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let available = global.try_lock().is_ok();
    let admitted = available && require_business().is_ok();
    release_tx.send(()).unwrap();
    server.join().unwrap();
    assert!(request.join().unwrap().is_err());
    *global.lock().unwrap() = original;
    assert!(
        admitted,
        "HTTP等待期间必须释放授权状态锁，允许已有应急授权继续准入"
    );
}

#[test]
#[ignore = "requires an explicitly provisioned disposable Grantmint license and local server"]
fn grantmint_live_activation_secure_restart_sync_and_acknowledgement() {
    let path =
        std::env::var("GLUESIGHT_LICENSE_E2E_CONFIG").expect("set disposable E2E fixture path");
    let fixture: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let config: Config = serde_json::from_value(fixture["config"].clone()).unwrap();
    let dir = Temp::new();
    let mut manager = LicenseManager::open(config.clone(), dir.0.clone()).unwrap();
    let license_id = fixture["licenseId"].as_str().unwrap();
    let secret = fixture["activationSecret"].as_str().unwrap();
    let activated = manager.activate(license_id, secret).unwrap();
    assert!(activated.allowed);
    assert!(!activated.message.contains("尚未确认"));
    assert!(!fs::read(dir.0.join("state.bin"))
        .unwrap()
        .windows(secret.len())
        .any(|window| window == secret.as_bytes()));
    drop(manager);
    let mut reopened = LicenseManager::open(config.clone(), dir.0.clone()).unwrap();
    assert!(reopened.require_business().is_ok());
    let admin = &fixture["admin"];
    let client = reqwest::blocking::Client::new();
    let login = client
        .post(format!("{}/admin/auth/login", config.server_url))
        .json(&json!({"username":admin["username"],"password":admin["password"]}))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap();
    let cookie = login
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let csrf = login.json::<serde_json::Value>().unwrap()["csrfToken"]
        .as_str()
        .unwrap()
        .to_string();
    let renewed_expiry = timestamp(utc_now() + 730 * 86400).unwrap();
    let renewed:serde_json::Value=client.post(format!("{}/admin/v2/licenses/{license_id}/renewals",config.server_url)).header(reqwest::header::COOKIE,&cookie).header("X-XSRF-TOKEN",&csrf).json(&json!({"expectedRevision":1,"newExpiresAt":renewed_expiry,"idempotencyKey":uuid::Uuid::new_v4().to_string(),"reason":"GlueSight DPAPI actual renewal verification"})).send().unwrap().error_for_status().unwrap().json().unwrap();
    assert_eq!(renewed["revision"], 2);
    let synced = reopened.sync().unwrap();
    assert!(synced.allowed);
    assert!(!synced.message.contains("尚未确认"));
    assert_eq!(synced.license_id.as_deref(), Some(license_id));
    assert_ne!(synced.expires_at, activated.expires_at);
    assert_eq!(synced.expires_at.as_deref(), Some(renewed_expiry.as_str()));
    let detail: serde_json::Value = client
        .get(format!(
            "{}/admin/v2/licenses/{license_id}",
            config.server_url
        ))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(detail["deliveryStatus"], "confirmed");
    drop(reopened);
    let mut offline_config = config.clone();
    offline_config.server_url = "http://127.0.0.1:9".into();
    let mut offline = LicenseManager::open(offline_config, dir.0.clone()).unwrap();
    assert!(offline.require_business().is_ok());
    assert!(offline.sync().is_err());
    assert_eq!(offline.status().expires_at, synced.expires_at);
    drop(offline);
    let mut reopened = LicenseManager::open(config, dir.0.clone()).unwrap();
    assert_eq!(reopened.status().expires_at, synced.expires_at);
    assert!(reopened.require_business().is_ok());
}

#[test]
fn failed_anchor_replace_rolls_back_new_certificate_and_preserves_old_expiry() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = Temp::new();
    let now = utc_now();
    let mut manager = open_clock(config(), dir.0.clone());
    let first = signed_bundle(&manager, now, now + 86400, 1);
    install(&mut manager, first);
    let previous = manager.status();
    let locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(dir.0.join("anchor.bin"))
        .unwrap();
    let next = signed_bundle(&manager, now, now + 30 * 86400, 2);
    assert!(manager.install_test_fixture(next).is_err());
    drop(locked);
    assert_eq!(manager.status().expires_at, previous.expires_at);
    assert!(manager.require_business().is_ok());
}
