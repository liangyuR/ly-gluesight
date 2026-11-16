use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

pub struct Store {
    root: PathBuf,
    _lease: File,
}

impl Store {
    pub fn open(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&root).map_err(|_| "不能创建授权安全存储目录")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .map_err(|_| "无法限制授权目录权限")?;
        }
        if fs::symlink_metadata(&root)
            .map_err(|_| "无法检查授权目录")?
            .file_type()
            .is_symlink()
        {
            return Err("授权目录不能是符号链接".into());
        }
        let lease = secure_options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("instance.lock"))
            .map_err(|_| "无法打开授权进程锁")?;
        lease
            .try_lock()
            .map_err(|_| "授权存储正在被另一个 GlueSight 进程使用")?;
        Ok(Self {
            root,
            _lease: lease,
        })
    }

    pub fn exists(&self, name: &str) -> bool {
        self.root.join(name).exists()
    }

    pub fn read<T: DeserializeOwned>(&self, name: &str) -> Result<T, String> {
        let path = self.root.join(name);
        if fs::symlink_metadata(&path)
            .map_err(|_| "授权保护文件缺失")?
            .file_type()
            .is_symlink()
        {
            return Err("授权保护文件不能是链接".into());
        }
        let mut bytes = Vec::new();
        File::open(&path)
            .map_err(|_| "无法读取授权保护文件")?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "无法读取授权保护文件")?;
        if bytes.len() > 1024 * 1024 {
            return Err("授权保护文件过大".into());
        }
        let plain = Zeroizing::new(unprotect(&bytes)?);
        serde_json::from_slice(&plain).map_err(|_| "授权保护文件内容损坏".into())
    }

    pub fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        let plain = Zeroizing::new(serde_json::to_vec(value).map_err(|_| "无法编码授权状态")?);
        let bytes = protect(&plain)?;
        let pending = self
            .root
            .join(format!(".{}-{}.pending", name, uuid::Uuid::new_v4()));
        let target = self.root.join(name);
        let result = (|| {
            let mut file = secure_options()
                .write(true)
                .create_new(true)
                .open(&pending)
                .map_err(|_| "无法写入授权保护文件")?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| "无法可靠保存授权保护文件")?;
            drop(file);
            replace(&pending, &target)?;
            #[cfg(unix)]
            File::open(&self.root)
                .and_then(|file| file.sync_all())
                .map_err(|_| "无法同步授权目录")?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(pending);
        }
        result
    }
}

fn secure_options() -> OpenOptions {
    let options = OpenOptions::new();
    #[cfg(unix)]
    let options = {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = options;
        options.mode(0o600);
        options
    };
    options
}

#[cfg(windows)]
fn replace(source: &Path, target: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err("无法原子替换授权保护文件".into())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace(source: &Path, target: &Path) -> Result<(), String> {
    fs::rename(source, target).map_err(|_| "无法原子替换授权保护文件".into())
}

#[cfg(windows)]
fn crypt(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let entropy = b"GlueSight.license.v2.current-user";
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: entropy.len() as u32,
        pbData: entropy.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err("DPAPI 无法读取或保护授权状态，请使用原 Windows 账号并联系维护人员".into());
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        if !encrypt {
            std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
        }
        LocalFree(output.pbData.cast());
    }
    Ok(result)
}
#[cfg(windows)]
fn protect(bytes: &[u8]) -> Result<Vec<u8>, String> {
    crypt(bytes, true)
}
#[cfg(windows)]
fn unprotect(bytes: &[u8]) -> Result<Vec<u8>, String> {
    crypt(bytes, false)
}

#[cfg(not(windows))]
fn protect(_bytes: &[u8]) -> Result<Vec<u8>, String> {
    Err("当前发行版仅支持 Windows DPAPI 授权安全存储".into())
}
#[cfg(not(windows))]
fn unprotect(_bytes: &[u8]) -> Result<Vec<u8>, String> {
    Err("当前发行版仅支持 Windows DPAPI 授权安全存储".into())
}

#[cfg(windows)]
pub fn directory() -> Result<PathBuf, String> {
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath},
    };
    let mut path = std::ptr::null_mut();
    if unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, std::ptr::null_mut(), &mut path) }
        != 0
    {
        return Err("无法取得当前用户授权目录".into());
    }
    let mut len = 0;
    unsafe {
        while *path.add(len) != 0 {
            len += 1;
        }
    }
    let result =
        PathBuf::from(unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(path, len)) });
    unsafe {
        CoTaskMemFree(path.cast());
    }
    Ok(result.join("GlueSight").join("licensing"))
}
#[cfg(not(windows))]
pub fn directory() -> Result<PathBuf, String> {
    Err("当前发行版仅支持 Windows 授权安全存储".into())
}
