//! 海康 MVS 工业相机 SDK（MvCameraControl）的最小绑定。运行时动态加载，未安装 SDK 时软件仍可用模拟相机运行。

use std::ffi::{c_char, c_uint, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use libloading::Library;
use serde::Serialize;

#[cfg(windows)]
mod windows;

pub type Handle = *mut c_void;
pub type ImageCallback = extern "system" fn(*mut u8, *mut FrameInfo, *mut c_void);
pub type ExceptionCallback = extern "system" fn(c_uint, *mut c_void);

pub const MV_GIGE_DEVICE: c_uint = 0x1;
pub const MV_USB_DEVICE: c_uint = 0x4;
pub const MV_ACCESS_EXCLUSIVE: c_uint = 1;
pub const MV_EXCEPTION_DEV_DISCONNECT: c_uint = 0x8001;
pub const PIXEL_MONO8: u32 = 0x0108_0001;
pub const PIXEL_BAYER_GR8: u32 = 0x0108_0008;
pub const PIXEL_BAYER_RG8: u32 = 0x0108_0009;
pub const PIXEL_BAYER_GB8: u32 = 0x0108_000A;
pub const PIXEL_BAYER_BG8: u32 = 0x0108_000B;

#[repr(C)]
struct DeviceInfoHead {
    major_ver: u16,
    minor_ver: u16,
    mac_high: u32,
    mac_low: u32,
    tlayer_type: u32,
    dev_type_info: u32,
    reserved: [u32; 3],
}

#[repr(C)]
struct GigeInfo {
    ip_cfg_option: u32,
    ip_cfg_current: u32,
    current_ip: u32,
    subnet_mask: u32,
    gateway: u32,
    manufacturer: [u8; 32],
    model: [u8; 32],
    version: [u8; 32],
    specific: [u8; 48],
    serial: [u8; 16],
    user_name: [u8; 16],
}

#[repr(C)]
struct Usb3Info {
    endpoints: [u8; 4],
    vendor_id: u16,
    product_id: u16,
    device_number: u32,
    guid: [u8; 64],
    vendor: [u8; 64],
    model: [u8; 64],
    family: [u8; 64],
    version: [u8; 64],
    manufacturer: [u8; 64],
    serial: [u8; 64],
    user_name: [u8; 64],
}

#[repr(C)]
struct DeviceInfoList {
    count: c_uint,
    devices: [*const DeviceInfoHead; 256],
}

/// MV_FRAME_OUT_INFO_EX 的前缀，SDK 以指针传入，只读取到本帧丢包数为止。
#[repr(C)]
pub struct FrameInfo {
    pub width: u16,
    pub height: u16,
    pub pixel_type: u32,
    pub frame_num: u32,
    pub dev_ts_high: u32,
    pub dev_ts_low: u32,
    reserved0: u32,
    pub host_ts: i64,
    pub frame_len: u32,
    second_count: u32,
    cycle_count: u32,
    cycle_offset: u32,
    pub gain: f32,
    pub exposure: f32,
    pub brightness: u32,
    red: u32,
    green: u32,
    blue: u32,
    pub frame_counter: u32,
    pub trigger_index: u32,
    input: u32,
    output: u32,
    offset_x: u16,
    offset_y: u16,
    chunk_width: u16,
    chunk_height: u16,
    pub lost_packet: u32,
    unparsed_chunk_num: u32,
    unparsed_chunk: i64,
    pub extend_width: u32,
    pub extend_height: u32,
}

#[repr(C)]
struct FloatValue {
    cur: f32,
    max: f32,
    min: f32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Default)]
struct IntValue {
    cur: i64,
    max: i64,
    min: i64,
    inc: i64,
    reserved: [u32; 16],
}

#[repr(C)]
struct EnumValue {
    cur: c_uint,
    supported_num: c_uint,
    supported: [c_uint; 64],
    reserved: [u32; 4],
}

#[repr(C)]
struct EnumEntry {
    value: c_uint,
    symbolic: [c_char; 64],
    reserved: [u32; 4],
}

type FnInit = unsafe extern "system" fn() -> i32;
type FnEnum = unsafe extern "system" fn(c_uint, *mut DeviceInfoList) -> i32;
type FnCreate = unsafe extern "system" fn(*mut Handle, *const DeviceInfoHead) -> i32;
type FnOpen = unsafe extern "system" fn(Handle, c_uint, u16) -> i32;
type FnHandle = unsafe extern "system" fn(Handle) -> i32;
type FnRegImage = unsafe extern "system" fn(Handle, ImageCallback, *mut c_void) -> i32;
type FnRegExc = unsafe extern "system" fn(Handle, ExceptionCallback, *mut c_void) -> i32;
type FnSetEnumStr = unsafe extern "system" fn(Handle, *const c_char, *const c_char) -> i32;
type FnSetFloat = unsafe extern "system" fn(Handle, *const c_char, f32) -> i32;
type FnGetFloat = unsafe extern "system" fn(Handle, *const c_char, *mut FloatValue) -> i32;
type FnSetInt = unsafe extern "system" fn(Handle, *const c_char, i64) -> i32;
type FnGetInt = unsafe extern "system" fn(Handle, *const c_char, *mut IntValue) -> i32;
type FnGetNodeType = unsafe extern "system" fn(Handle, *const c_char, *mut i32) -> i32;
type FnGetBool = unsafe extern "system" fn(Handle, *const c_char, *mut bool) -> i32;
type FnSetBool = unsafe extern "system" fn(Handle, *const c_char, bool) -> i32;
type FnCommand = unsafe extern "system" fn(Handle, *const c_char) -> i32;
type FnGetEnum = unsafe extern "system" fn(Handle, *const c_char, *mut EnumValue) -> i32;
type FnGetEnumEntry = unsafe extern "system" fn(Handle, *const c_char, *mut EnumEntry) -> i32;
type FnVersion = unsafe extern "system" fn() -> c_uint;

pub struct Api {
    _lib: Library,
    #[cfg(windows)]
    _directories: windows::DllDirectories,
    finalize: FnInit,
    initialized: bool,
    enum_devices: FnEnum,
    create_handle: FnCreate,
    open_device: FnOpen,
    close_device: FnHandle,
    destroy_handle: FnHandle,
    start_grabbing: FnHandle,
    stop_grabbing: FnHandle,
    optimal_packet_size: FnHandle,
    register_image: FnRegImage,
    register_exception: FnRegExc,
    set_enum_str: FnSetEnumStr,
    set_float: FnSetFloat,
    get_float: FnGetFloat,
    set_int: FnSetInt,
    get_int: FnGetInt,
    get_node_type: FnGetNodeType,
    set_bool: FnSetBool,
    get_bool: FnGetBool,
    command: FnCommand,
    get_enum: FnGetEnum,
    get_enum_entry: FnGetEnumEntry,
    pub version: String,
    pub library_path: PathBuf,
}

#[cfg(any(windows, test))]
fn windows_candidates(executable: &std::path::Path, development: bool, common_files: Option<PathBuf>) -> Result<Vec<PathBuf>, String> {
    let directory = executable.parent().filter(|p| p.is_absolute()).ok_or("无法确定 MVS 随包运行库目录")?;
    let mut out = vec![directory.join("runtime").join("mvs").join("MvCameraControl.dll")];
    if development {
        if let Some(dir) = common_files.filter(|p| p.is_absolute()) {
            out.push(dir.join("MVS").join("Runtime").join("Win64_x64").join("MvCameraControl.dll"));
        }
        out.push(PathBuf::from(r"C:\Program Files (x86)\Common Files\MVS\Runtime\Win64_x64\MvCameraControl.dll"));
    }
    out.dedup();
    Ok(out)
}

fn candidates() -> Result<Vec<PathBuf>, String> {
    #[cfg(windows)]
    {
        let executable = std::env::current_exe().map_err(|e| format!("读取应用程序路径失败: {e}"))?;
        windows_candidates(&executable, cfg!(debug_assertions), std::env::var_os("CommonProgramFiles(x86)").map(PathBuf::from))
    }
    #[cfg(not(windows))]
    {
        Ok(vec![PathBuf::from("libMvCameraControl.so")])
    }
}

impl Api {
    fn load() -> Result<Self, String> {
        Self::load_candidates(candidates()?)
    }

    fn load_candidates(paths: Vec<PathBuf>) -> Result<Self, String> {
        let mut last = String::from("未找到 MvCameraControl");
        for path in paths {
            #[cfg(windows)]
            let result = windows::load(&path).and_then(|(lib, directories, loaded_path)| unsafe { Self::bind(lib, loaded_path, directories) });
            #[cfg(not(windows))]
            let result = unsafe { Library::new(&path) }.map_err(|e| e.to_string()).and_then(|lib| unsafe { Self::bind(lib, path.clone()) });
            match result {
                Ok(api) => return Ok(api),
                Err(e) => last = format!("{}: {e}", path.display()),
            }
        }
        Err(format!("海康 MVS 运行库加载失败（{last}）"))
    }

    unsafe fn bind(lib: Library, library_path: PathBuf, #[cfg(windows)] directories: windows::DllDirectories) -> Result<Self, String> {
        macro_rules! sym {
            ($name:literal) => {
                *lib.get(concat!($name, "\0").as_bytes()).map_err(|e| format!("MVS 缺少接口 {}: {e}", $name))?
            };
        }
        let initialize: FnInit = sym!("MV_CC_Initialize");
        let finalize: FnInit = sym!("MV_CC_Finalize");
        let version: FnVersion = sym!("MV_CC_GetSDKVersion");
        let mut api = Self {
            enum_devices: sym!("MV_CC_EnumDevices"),
            create_handle: sym!("MV_CC_CreateHandle"),
            open_device: sym!("MV_CC_OpenDevice"),
            close_device: sym!("MV_CC_CloseDevice"),
            destroy_handle: sym!("MV_CC_DestroyHandle"),
            start_grabbing: sym!("MV_CC_StartGrabbing"),
            stop_grabbing: sym!("MV_CC_StopGrabbing"),
            optimal_packet_size: sym!("MV_CC_GetOptimalPacketSize"),
            register_image: sym!("MV_CC_RegisterImageCallBackEx"),
            register_exception: sym!("MV_CC_RegisterExceptionCallBack"),
            set_enum_str: sym!("MV_CC_SetEnumValueByString"),
            set_float: sym!("MV_CC_SetFloatValue"),
            get_float: sym!("MV_CC_GetFloatValue"),
            set_int: sym!("MV_CC_SetIntValueEx"),
            get_int: sym!("MV_CC_GetIntValueEx"),
            get_node_type: sym!("MV_XML_GetNodeInterfaceType"),
            set_bool: sym!("MV_CC_SetBoolValue"),
            get_bool: sym!("MV_CC_GetBoolValue"),
            command: sym!("MV_CC_SetCommandValue"),
            get_enum: sym!("MV_CC_GetEnumValue"),
            get_enum_entry: sym!("MV_CC_GetEnumEntrySymbolic"),
            version: String::new(),
            library_path,
            finalize,
            initialized: false,
            _lib: lib,
            #[cfg(windows)]
            _directories: directories,
        };
        check(initialize(), "初始化 MVS SDK")?;
        api.initialized = true;
        let v = version();
        api.version = format!("{}.{}.{}.{}", v >> 24, (v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff);
        crate::applog::write("info", "MVS SDK", &format!("{}：{}", api.version, api.library_path.display()));
        Ok(api)
    }
}

impl Drop for Api {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { (self.finalize)(); }
        }
    }
}

pub fn api() -> Result<Arc<Api>, String> {
    static API: OnceLock<Result<Arc<Api>, String>> = OnceLock::new();
    API.get_or_init(|| Api::load().map(Arc::new)).clone()
}

pub(crate) fn load_bundled_api() -> Result<Arc<Api>, String> {
    #[cfg(windows)]
    {
        let executable = std::env::current_exe().map_err(|e| format!("读取应用程序路径失败: {e}"))?;
        Api::load_candidates(windows_candidates(&executable, false, None)?).map(Arc::new)
    }
    #[cfg(not(windows))]
    {
        Err("MVS 随包检查仅支持 Windows".into())
    }
}

fn check(code: i32, what: &str) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("{what} 失败（0x{:08X}）", code as u32))
    }
}

fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    pub serial: String,
    pub model: String,
    pub user_name: String,
    pub transport: &'static str,
    pub ip: Option<String>,
}

fn summarize(info: &DeviceInfoHead) -> DeviceSummary {
    let special = unsafe { (info as *const DeviceInfoHead).add(1) as *const u8 };
    if info.tlayer_type == MV_GIGE_DEVICE {
        let g = unsafe { &*(special as *const GigeInfo) };
        let ip = g.current_ip;
        DeviceSummary {
            serial: text(&g.serial),
            model: text(&g.model),
            user_name: text(&g.user_name),
            transport: "GigE",
            ip: Some(format!("{}.{}.{}.{}", ip >> 24, (ip >> 16) & 0xff, (ip >> 8) & 0xff, ip & 0xff)),
        }
    } else {
        let u = unsafe { &*(special as *const Usb3Info) };
        DeviceSummary {
            serial: text(&u.serial),
            model: text(&u.model),
            user_name: text(&u.user_name),
            transport: "USB3",
            ip: None,
        }
    }
}

fn enumerate_raw(api: &Api) -> Result<(Box<DeviceInfoList>, Vec<DeviceSummary>), String> {
    let mut list = Box::new(DeviceInfoList { count: 0, devices: [std::ptr::null(); 256] });
    check(unsafe { (api.enum_devices)(MV_GIGE_DEVICE | MV_USB_DEVICE, &mut *list) }, "枚举相机")?;
    let n = (list.count as usize).min(256);
    let summaries = list.devices[..n].iter().filter(|p| !p.is_null()).map(|&p| summarize(unsafe { &*p })).collect();
    Ok((list, summaries))
}

pub fn enumerate() -> Result<Vec<DeviceSummary>, String> {
    let api = api()?;
    enumerate_with_api(&api)
}

pub(crate) fn enumerate_with_api(api: &Api) -> Result<Vec<DeviceSummary>, String> {
    enumerate_raw(api).map(|(_, s)| s)
}

/// 已打开的相机。所有接口均为线程安全，句柄可跨线程使用。
pub struct Device {
    api: Arc<Api>,
    handle: Handle,
    pub summary: DeviceSummary,
}

unsafe impl Send for Device {}
unsafe impl Sync for Device {}

impl Device {
    /// 打开序列号匹配的相机。
    pub fn open(serial: &str) -> Result<Self, String> {
        if serial.trim().is_empty() {
            return Err("未绑定设备".into());
        }
        let api = api()?;
        let (list, summaries) = enumerate_raw(&api)?;
        let Some(index) = summaries.iter().position(|s| s.serial == serial) else {
            return Err(format!("未找到序列号为 {serial} 的相机"));
        };
        let mut handle: Handle = std::ptr::null_mut();
        check(unsafe { (api.create_handle)(&mut handle, list.devices[index]) }, "创建相机句柄")?;
        let device = Self { api, handle, summary: summaries[index].clone() };
        check(unsafe { (device.api.open_device)(handle, MV_ACCESS_EXCLUSIVE, 0) }, "打开相机（可能已被 MVS 客户端占用）")?;
        Ok(device)
    }

    fn key(name: &str) -> CString {
        CString::new(name).unwrap_or_default()
    }

    pub fn set_enum(&self, key: &str, value: &str) -> Result<(), String> {
        let v = Self::key(value);
        check(unsafe { (self.api.set_enum_str)(self.handle, Self::key(key).as_ptr(), v.as_ptr()) }, &format!("{key} = {value}"))
    }

    pub fn set_float(&self, key: &str, value: f32) -> Result<(), String> {
        check(unsafe { (self.api.set_float)(self.handle, Self::key(key).as_ptr(), value) }, &format!("{key} = {value}"))
    }

    pub fn get_float(&self, key: &str) -> Result<f32, String> {
        let mut v = FloatValue { cur: 0.0, max: 0.0, min: 0.0, reserved: [0; 4] };
        check(unsafe { (self.api.get_float)(self.handle, Self::key(key).as_ptr(), &mut v) }, &format!("读取 {key}"))?;
        Ok(v.cur)
    }

    pub fn set_int(&self, key: &str, value: i64) -> Result<(), String> {
        check(unsafe { (self.api.set_int)(self.handle, Self::key(key).as_ptr(), value) }, &format!("{key} = {value}"))
    }

    pub fn get_int(&self, key: &str) -> Result<i64, String> {
        let mut value = IntValue::default();
        check(unsafe { (self.api.get_int)(self.handle, Self::key(key).as_ptr(), &mut value) }, &format!("读取 {key}"))?;
        Ok(value.cur)
    }

    pub fn has_node(&self, key: &str) -> Result<bool, String> {
        let mut node_type = 0;
        let code = unsafe { (self.api.get_node_type)(self.handle, Self::key(key).as_ptr(), &mut node_type) };
        if code as u32 == 0x80000109 { return Ok(false); }
        check(code, &format!("读取 {key} 节点类型"))?;
        Ok(true)
    }

    pub fn set_trigger_delay(&self, value: f32) -> Result<(), String> {
        let key = "TriggerDelay";
        if !value.is_finite() || value < 0.0 {
            return Err("触发延迟必须是非负有限数值".into());
        }
        let mut node_type = 0;
        check(unsafe { (self.api.get_node_type)(self.handle, Self::key(key).as_ptr(), &mut node_type) }, "读取 TriggerDelay 节点类型")?;
        match node_type {
            2 => {
                if value.fract() != 0.0 || value as f64 >= i64::MAX as f64 {
                    return Err("当前相机的触发延迟仅支持整数微秒".into());
                }
                let expected = value as i64;
                self.set_int(key, expected)?;
                let actual = self.get_int(key)?;
                if actual != expected {
                    return Err(format!("TriggerDelay 回读 {actual} µs，期望 {expected} µs"));
                }
            }
            5 => {
                self.set_float(key, value)?;
                let actual = self.get_float(key)?;
                if !actual.is_finite() || (actual - value).abs() > (value.abs() * f32::EPSILON * 4.0).max(0.001) {
                    return Err(format!("TriggerDelay 回读 {actual} µs，期望 {value} µs"));
                }
            }
            _ => return Err(format!("TriggerDelay 节点类型 {node_type} 不是 Integer 或 Float")),
        }
        Ok(())
    }

    pub fn set_bool(&self, key: &str, value: bool) -> Result<(), String> {
        check(unsafe { (self.api.set_bool)(self.handle, Self::key(key).as_ptr(), value) }, &format!("{key} = {value}"))
    }

    pub fn get_bool(&self, key: &str) -> Result<bool, String> {
        let mut value = false;
        check(unsafe { (self.api.get_bool)(self.handle, Self::key(key).as_ptr(), &mut value) }, &format!("读取 {key}"))?;
        Ok(value)
    }

    pub fn get_enum(&self, key: &str) -> Result<String, String> {
        let k = Self::key(key);
        let mut v = EnumValue { cur: 0, supported_num: 0, supported: [0; 64], reserved: [0; 4] };
        check(unsafe { (self.api.get_enum)(self.handle, k.as_ptr(), &mut v) }, &format!("读取 {key}"))?;
        let mut entry = EnumEntry { value: v.cur, symbolic: [0; 64], reserved: [0; 4] };
        check(unsafe { (self.api.get_enum_entry)(self.handle, k.as_ptr(), &mut entry) }, &format!("读取 {key} 当前枚举"))?;
        Ok(unsafe { CStr::from_ptr(entry.symbolic.as_ptr()) }.to_string_lossy().into_owned())
    }

    pub fn command(&self, key: &str) -> Result<(), String> {
        check(unsafe { (self.api.command)(self.handle, Self::key(key).as_ptr()) }, key)
    }

    /// 枚举节点当前支持的全部取值名称。
    pub fn enum_entries(&self, key: &str) -> Result<Vec<String>, String> {
        let k = Self::key(key);
        let mut v = EnumValue { cur: 0, supported_num: 0, supported: [0; 64], reserved: [0; 4] };
        check(unsafe { (self.api.get_enum)(self.handle, k.as_ptr(), &mut v) }, &format!("读取 {key}"))?;
        let mut out = Vec::new();
        for &value in &v.supported[..(v.supported_num as usize).min(64)] {
            let mut e = EnumEntry { value, symbolic: [0; 64], reserved: [0; 4] };
            if unsafe { (self.api.get_enum_entry)(self.handle, k.as_ptr(), &mut e) } == 0 {
                out.push(unsafe { CStr::from_ptr(e.symbolic.as_ptr()) }.to_string_lossy().into_owned());
            }
        }
        Ok(out)
    }

    pub fn optimal_packet_size(&self) -> Option<i64> {
        let size = unsafe { (self.api.optimal_packet_size)(self.handle) };
        (size > 0).then_some(size as i64)
    }

    /// `user` 必须在相机关闭前保持有效。
    pub fn start(&self, on_image: ImageCallback, on_exception: ExceptionCallback, user: *mut c_void) -> Result<(), String> {
        check(unsafe { (self.api.register_exception)(self.handle, on_exception, user) }, "注册异常回调")?;
        check(unsafe { (self.api.register_image)(self.handle, on_image, user) }, "注册取图回调")?;
        check(unsafe { (self.api.start_grabbing)(self.handle) }, "开始取流")
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            (self.api.stop_grabbing)(self.handle);
            (self.api.close_device)(self.handle);
            (self.api.destroy_handle)(self.handle);
        }
    }
}

#[cfg(test)]
mod runtime_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn windows_candidates_follow_packaged_and_development_policy() {
        let executable = std::env::temp_dir().join("GlueSight.exe");
        let system = std::env::temp_dir().join("system-sdk");
        let bundled = executable.parent().unwrap().join("runtime").join("mvs").join("MvCameraControl.dll");
        let system_dll = system.join("MVS").join("Runtime").join("Win64_x64").join("MvCameraControl.dll");
        type Check<'a> = Box<dyn Fn(&Result<Vec<PathBuf>, String>) -> bool + 'a>;
        // (场景, 程序路径, 开发模式, 系统公共目录, 结果判定)
        let cases: [(&str, &Path, bool, Option<PathBuf>, Check<'_>); 4] = [
            ("安装包忽略系统安装", &executable, false, Some(system.clone()), Box::new(|r| *r == Ok(vec![bundled.clone()]))),
            ("开发模式支持绝对路径的系统安装", &executable, true, Some(system.clone()),
                Box::new(|r| r.as_ref().is_ok_and(|p| p.get(1) == Some(&system_dll) && !p.contains(&PathBuf::from("MvCameraControl.dll"))))),
            ("相对程序路径被拒绝", Path::new("relative/GlueSight.exe"), false, None, Box::new(|r| r.is_err())),
            ("相对系统目录不搜索", &executable, true, Some(PathBuf::from("relative-sdk")),
                Box::new(|r| r.as_ref().is_ok_and(|p| !p.iter().any(|p| p.starts_with("relative-sdk"))))),
        ];
        for (case, executable, development, system, check) in cases {
            let result = windows_candidates(executable, development, system);
            assert!(check(&result), "{case}: {result:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn missing_bundled_sdk_fails_even_with_system_sdk_present() {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("gluesight-missing-mvs-{}-{nonce}", std::process::id()));
        let executable = root.join("app").join("GlueSight.exe");
        let bundled = root.join("app").join("runtime").join("mvs").join("MvCameraControl.dll");
        // 造一个“已安装”的系统 SDK，结果不再取决于本机装没装 MVS；安装包若去试它，最后报的就是它的路径
        let common_files = root.join("common-files");
        let system_dll = common_files.join("MVS").join("Runtime").join("Win64_x64").join("MvCameraControl.dll");
        std::fs::create_dir_all(system_dll.parent().unwrap()).unwrap();
        std::fs::write(&system_dll, b"fake system MVS runtime").unwrap();
        let paths = windows_candidates(&executable, false, Some(common_files)).unwrap();
        let result = Api::load_candidates(paths);
        let _ = std::fs::remove_dir_all(&root);
        let error = match result {
            Ok(_) => panic!("missing packaged SDK must not load a system installation"),
            Err(error) => error,
        };
        assert!(error.contains(&bundled.display().to_string()), "{error}");
    }
}
