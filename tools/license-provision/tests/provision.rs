use argon2::{Argon2, PasswordHash, PasswordVerifier};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "gluesight-provision-test-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn config(&self, value: &Value) -> PathBuf {
        let path = self.0.join("input.json");
        std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config() -> Value {
    json!({"serverUrl":"https://license.example.invalid", "productId":"d4bf84c1-4f45-4826-80f0-3a16422225d6", "trustedKeys":{"test-key":STANDARD.encode([7;32])}, "emergencyPasswordHash":""})
}

fn check(path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gluesight-license-provision"))
        .arg("--check-release")
        .arg(path)
        .output()
        .unwrap()
}

#[test]
fn unknown_secret_field_is_rejected_without_echoing_its_value() {
    let fixture = Fixture::new();
    let mut value = config();
    value["emergencyPassword"] = "not-a-real-password-do-not-echo".into();
    let output = check(&fixture.config(&value));
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("not-a-real-password-do-not-echo"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("not-a-real-password-do-not-echo"));
}

#[test]
fn release_gate_rejects_placeholders_credentials_bad_keys_and_unsafe_password_hashes() {
    let fixture = Fixture::new();
    let example =
        serde_json::from_str(include_str!("../../../packaging/license.example.json")).unwrap();
    assert!(!check(&fixture.config(&example)).status.success());
    let cases = [
        ("/serverUrl", "http://127.0.0.1:8180"),
        ("/serverUrl", "https://user:password@example.invalid"),
        ("/serverUrl", "https://example.invalid/?password=secret"),
        ("/serverUrl", "https://example.invalid/#secret"),
        ("/productId", "invalid-uuid"),
        ("/productId", "D4BF84C1-4F45-4826-80F0-3A16422225D6"),
        ("/productId", "d4bf84c14f45482680f03a16422225d6"),
        ("/productId", "00000000-0000-0000-0000-000000000000"),
        ("/trustedKeys/test-key", "dGVzdA=="),
        ("/emergencyPasswordHash", "plaintext-password"),
        ("/emergencyPasswordHash", "$argon2id$v=19$m=8192,t=1,p=1"),
        ("/emergencyPasswordHash", "$argon2id$v=19$m=999999999,t=1,p=1$c29tZXNhbHRzb21lc2FsdA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
    ];
    for (field, invalid) in cases {
        let mut value = config();
        *value.pointer_mut(field).unwrap() = invalid.into();
        assert!(
            !check(&fixture.config(&value)).status.success(),
            "must reject {field}"
        );
    }
}

#[test]
fn generated_password_is_only_in_separate_file_and_repeated_generation_cannot_replace_it() {
    let fixture = Fixture::new();
    let input = fixture.config(&config());
    let destination = fixture.0.join("output");
    std::fs::create_dir(&destination).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_gluesight-license-provision"))
            .arg(&input)
            .arg(&destination)
            .output()
            .unwrap()
    };
    let output = run();
    assert!(output.status.success());
    let password = std::fs::read_to_string(destination.join("emergency-password.txt")).unwrap();
    let generated = std::fs::read(destination.join("license.local.json")).unwrap();
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&password));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(&password));
    assert!(!String::from_utf8_lossy(&generated).contains(&password));
    let value: Value = serde_json::from_slice(&generated).unwrap();
    let hash = PasswordHash::new(value["emergencyPasswordHash"].as_str().unwrap()).unwrap();
    assert!(Argon2::default()
        .verify_password(password.as_bytes(), &hash)
        .is_ok());
    assert!(check(&destination.join("license.local.json"))
        .status
        .success());
    assert!(!run().status.success());
    assert!(
        std::fs::read_to_string(destination.join("emergency-password.txt")).unwrap() == password
    );
    assert!(std::fs::read(destination.join("license.local.json")).unwrap() == generated);
}
