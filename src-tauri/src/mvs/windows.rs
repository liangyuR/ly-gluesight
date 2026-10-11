use std::ffi::{c_void, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use libloading::os::windows::{Library as WindowsLibrary, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LOAD_LIBRARY_SEARCH_USER_DIRS};
use libloading::Library;

#[link(name = "kernel32")]
extern "system" {
    fn AddDllDirectory(directory: *const u16) -> *mut c_void;
    fn RemoveDllDirectory(cookie: *mut c_void) -> i32;
    fn GetModuleFileNameW(module: isize, filename: *mut u16, size: u32) -> u32;
}

pub(super) struct DllDirectories(Vec<usize>);

impl DllDirectories {
    fn add(&mut self, directory: &Path) -> Result<(), String> {
        let wide: Vec<u16> = directory.as_os_str().encode_wide().chain(Some(0)).collect();
        let cookie = unsafe { AddDllDirectory(wide.as_ptr()) };
        if cookie.is_null() {
            return Err(format!("注册 MVS 依赖目录 {} 失败: {}", directory.display(), std::io::Error::last_os_error()));
        }
        self.0.push(cookie as usize);
        Ok(())
    }

    fn add_tree(&mut self, directory: &Path) -> Result<(), String> {
        self.add(directory)?;
        let entries = std::fs::read_dir(directory).map_err(|e| format!("读取 MVS 依赖目录 {} 失败: {e}", directory.display()))?;
        let mut children = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| format!("读取 MVS 依赖目录项失败: {e}"))?;
            if entry.file_type().map_err(|e| format!("读取 MVS 依赖类型失败: {e}"))?.is_dir() {
                children.push(entry.path());
            }
        }
        children.sort();
        for child in children {
            self.add_tree(&child)?;
        }
        Ok(())
    }
}

impl Drop for DllDirectories {
    fn drop(&mut self) {
        for cookie in self.0.drain(..).rev() {
            unsafe { RemoveDllDirectory(cookie as *mut c_void); }
        }
    }
}

pub(super) fn load(path: &Path) -> Result<(Library, DllDirectories, PathBuf), String> {
    if !path.is_absolute() {
        return Err("MVS 运行库必须使用绝对路径".into());
    }
    let path = path.canonicalize().map_err(|e| format!("MVS 随包运行库不存在或不可访问: {e}"))?;
    let root = path.parent().ok_or("MVS 运行库没有父目录")?;
    let mut directories = DllDirectories(Vec::new());
    directories.add(root)?;
    let third_party = root.join("ThirdParty");
    if third_party.is_dir() {
        directories.add_tree(&third_party)?;
    }
    let library = unsafe {
        WindowsLibrary::load_with_flags(&path, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_USER_DIRS | LOAD_LIBRARY_SEARCH_SYSTEM32)
    }.map_err(|e| e.to_string())?;
    let handle = library.into_raw();
    let library = unsafe { WindowsLibrary::from_raw(handle) };
    let mut buffer = vec![0u16; 32768];
    let length = unsafe { GetModuleFileNameW(handle as isize, buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(format!("读取 MVS 实际加载路径失败: {}", std::io::Error::last_os_error()));
    }
    let loaded = PathBuf::from(OsString::from_wide(&buffer[..length]));
    let loaded = loaded.canonicalize().map_err(|e| format!("解析 MVS 实际加载路径失败: {e}"))?;
    if loaded != path {
        return Err(format!("MVS 实际加载路径与指定运行库不一致: {}", loaded.display()));
    }
    Ok((library.into(), directories, loaded))
}
