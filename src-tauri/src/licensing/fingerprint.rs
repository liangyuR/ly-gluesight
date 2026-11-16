use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
pub struct Fingerprint {
    pub with_firmware: bool,
    pub hash: String,
}

impl Fingerprint {
    pub fn collect(product: &str, fixed: Option<bool>) -> Result<Self, String> {
        let (installation, firmware) = identifiers()?;
        Self::from_identifiers(product, &installation, firmware.as_deref(), fixed)
    }

    pub fn from_identifiers(
        product: &str,
        installation: &str,
        firmware: Option<&str>,
        fixed: Option<bool>,
    ) -> Result<Self, String> {
        let installation = normalize(installation).ok_or("无法读取有效的系统安装标识")?;
        let firmware = firmware.and_then(normalize);
        let with_firmware = fixed.unwrap_or(firmware.is_some());
        if with_firmware && firmware.is_none() {
            return Err("原绑定的主板标识不可读取，请联系维护人员".into());
        }
        let mut hash = Sha256::new();
        for value in [
            "gluesight.fingerprint.v1",
            product,
            &installation,
            if with_firmware {
                firmware.as_deref().unwrap()
            } else {
                "installation-only"
            },
        ] {
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
        Ok(Self {
            with_firmware,
            hash: URL_SAFE_NO_PAD.encode(hash.finalize()),
        })
    }
}

fn normalize(value: &str) -> Option<String> {
    let normalized: String = value
        .trim()
        .chars()
        .filter(|c| *c != '-' && *c != '{' && *c != '}')
        .flat_map(char::to_lowercase)
        .collect();
    (normalized.len() == 32
        && normalized.chars().all(|c| c.is_ascii_hexdigit())
        && normalized.chars().any(|c| c != '0')
        && normalized.chars().any(|c| c != 'f'))
    .then_some(normalized)
}

#[cfg(windows)]
fn identifiers() -> Result<(String, Option<String>), String> {
    use windows_sys::Win32::System::{
        Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY},
        SystemInformation::GetSystemFirmwareTable,
    };
    let key: Vec<u16> = "SOFTWARE\\Microsoft\\Cryptography\0"
        .encode_utf16()
        .collect();
    let name: Vec<u16> = "MachineGuid\0".encode_utf16().collect();
    let mut buffer = [0u16; 256];
    let mut size = (buffer.len() * 2) as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if result != 0 {
        return Err("无法读取系统安装标识".into());
    }
    let installation = String::from_utf16_lossy(
        &buffer[..buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len())],
    );
    let provider = u32::from_be_bytes(*b"RSMB");
    let len = unsafe { GetSystemFirmwareTable(provider, 0, std::ptr::null_mut(), 0) };
    let firmware = if (8..=1024 * 1024).contains(&len) {
        let mut data = vec![0; len as usize];
        if unsafe { GetSystemFirmwareTable(provider, 0, data.as_mut_ptr(), len) } == len {
            firmware_uuid(&data)
        } else {
            None
        }
    } else {
        None
    };
    Ok((installation, firmware))
}

#[cfg(windows)]
fn firmware_uuid(data: &[u8]) -> Option<String> {
    let mut offset = 8;
    while offset + 4 <= data.len() {
        let kind = data[offset];
        let len = data[offset + 1] as usize;
        if len < 4 || offset + len > data.len() {
            return None;
        }
        if kind == 1 && len >= 24 {
            return Some(
                data[offset + 8..offset + 24]
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            );
        }
        offset += len;
        while offset + 1 < data.len() && (data[offset] != 0 || data[offset + 1] != 0) {
            offset += 1;
        }
        offset += 2;
    }
    None
}

#[cfg(target_os = "linux")]
fn identifiers() -> Result<(String, Option<String>), String> {
    let installation =
        std::fs::read_to_string("/etc/machine-id").map_err(|_| "无法读取 /etc/machine-id")?;
    let firmware = std::fs::read_to_string("/sys/class/dmi/id/product_uuid").ok();
    Ok((installation, firmware))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn identifiers() -> Result<(String, Option<String>), String> {
    Err("当前平台尚未支持授权设备绑定".into())
}
