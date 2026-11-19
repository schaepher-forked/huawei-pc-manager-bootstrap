// wlan_fix.rs - patch for huawei-pc-manager-bootstrap
//
// Purpose: HiConnectivityService calls WlanQueryInterface with an uninitialized GUID
// (observed as a GUID with a fixed first 8 bytes and heap-garbage last 8 bytes).
// WlanSvc returns 1168
// (ERROR_NOT_FOUND), PCManager falls back to "wifi switch state = 2", and
// multi-screen collaboration aborts with -27 before it even starts.
//
// This module detours WlanQueryInterface and replaces the GUID argument with the
// first real WLAN interface GUID reported by WlanEnumInterfaces.
//
// Integration:
//   1) put this file at packages/common/src/wlan_fix.rs
//   2) packages/common/src/lib.rs   ->  pub mod wlan_fix;
//   3) packages/common/src/common.rs ->  inside pub fn enable_hook(...), before Ok(())
//        crate::wlan_fix::install_async();
//
// Only HiConnectivityService.exe installs the hook, so wlanapi.dll is not force-loaded
// into every PCManager process.

use std::ffi::{c_void, CString};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use retour::static_detour;
use tracing::{info, warn};
use widestring::U16CString;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Guid {
    pub data: [u8; 16],
}

#[repr(C)]
struct WlanInterfaceInfo {
    interface_guid: Guid,
    description: [u16; 256],
    is_state: u32,
}

#[repr(C)]
struct WlanInterfaceInfoList {
    number_of_items: u32,
    index: u32,
    // WLAN_INTERFACE_INFO InterfaceInfo[1];
}

type FnWlanOpenHandle =
    unsafe extern "system" fn(u32, *mut c_void, *mut u32, *mut *mut c_void) -> u32;
type FnWlanEnumInterfaces =
    unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut WlanInterfaceInfoList) -> u32;
type FnWlanFreeMemory = unsafe extern "system" fn(*mut c_void);
type FnWlanQueryInterface = unsafe extern "system" fn(
    *mut c_void,      // hClientHandle
    *mut Guid,        // pInterfaceGuid
    u32,              // OpCode
    *mut c_void,      // pReserved
    *mut u32,         // pdwDataSize
    *mut *mut c_void, // ppData
    *mut u32,         // pWlanOpcodeValueType
) -> u32;

static_detour! {
    static HookWlanQueryInterface: unsafe extern "system" fn(
        *mut c_void, *mut Guid, u32, *mut c_void, *mut u32, *mut *mut c_void, *mut u32
    ) -> u32;
}

static PHYSICAL: Mutex<Option<Guid>> = Mutex::new(None);

fn is_target_process() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_lowercase()))
        .map(|n| n.starts_with("hiconnectivityservice"))
        .unwrap_or(false)
}

fn load_wlanapi() -> *mut c_void {
    match U16CString::from_str("wlanapi.dll") {
        Ok(name) => unsafe { LoadLibraryW(name.as_ptr()) as *mut c_void },
        Err(_) => std::ptr::null_mut(),
    }
}

unsafe fn export(module: *mut c_void, name: &str) -> Option<*const c_void> {
    let cname = CString::new(name).ok()?;
    let f = GetProcAddress(module as _, cname.as_ptr() as _)?;
    Some(f as usize as *const c_void)
}

/// Resolve (and cache) the GUID of the first WLAN interface.
fn physical_guid() -> Option<Guid> {
    if let Ok(guard) = PHYSICAL.lock() {
        if let Some(g) = *guard {
            return Some(g);
        }
    }

    let module = load_wlanapi();
    if module.is_null() {
        return None;
    }

    unsafe {
        let open: FnWlanOpenHandle = std::mem::transmute(export(module, "WlanOpenHandle")?);
        let enum_ifaces: FnWlanEnumInterfaces =
            std::mem::transmute(export(module, "WlanEnumInterfaces")?);
        let free: FnWlanFreeMemory = std::mem::transmute(export(module, "WlanFreeMemory")?);

        let mut negotiated = 0u32;
        let mut client: *mut c_void = std::ptr::null_mut();
        if open(2, std::ptr::null_mut(), &mut negotiated, &mut client) != 0 {
            return None;
        }

        let mut list: *mut WlanInterfaceInfoList = std::ptr::null_mut();
        if enum_ifaces(client, std::ptr::null_mut(), &mut list) != 0 || list.is_null() {
            return None;
        }

        let count = (*list).number_of_items;
        let mut found: Option<Guid> = None;
        if count > 0 {
            // WLAN_INTERFACE_INFO_LIST: DWORD dwNumberOfItems; DWORD dwIndex; INFO[]
            let first = (list as *mut u8).add(8) as *mut WlanInterfaceInfo;
            found = Some((*first).interface_guid);
        }
        free(list as *mut c_void);

        if let Some(g) = found {
            if let Ok(mut guard) = PHYSICAL.lock() {
                *guard = Some(g);
            }
            info!("wlan_fix: physical WLAN GUID = {:02x?}", g.data);
        }
        found
    }
}

unsafe extern "system" fn detour_query(
    client: *mut c_void,
    guid: *mut Guid,
    op_code: u32,
    reserved: *mut c_void,
    data_size: *mut u32,
    data: *mut *mut c_void,
    value_type: *mut u32,
) -> u32 {
    if !guid.is_null() {
        if let Some(physical) = physical_guid() {
            if (*guid).data != physical.data {
                *guid = physical;
            }
        }
    }
    HookWlanQueryInterface.call(client, guid, op_code, reserved, data_size, data, value_type)
}

pub fn install_async() {
    if !is_target_process() {
        return;
    }
    thread::spawn(|| {
        for attempt in 0..600u32 {
            let module = load_wlanapi();
            if !module.is_null() {
                unsafe {
                    if let Some(addr) = export(module, "WlanQueryInterface") {
                        let f: FnWlanQueryInterface = std::mem::transmute(addr);
                        // retour 0.3 的 initialize 要求安全闭包；detour_query 是
                        // unsafe fn，需要在这里包一层。
                        let detour =
                            |client, guid, op_code, reserved, data_size, data, value_type| {
                                detour_query(
                                    client, guid, op_code, reserved, data_size, data, value_type,
                                )
                            };
                        match HookWlanQueryInterface.initialize(f, detour) {
                            Ok(_) => match HookWlanQueryInterface.enable() {
                                Ok(()) => {
                                    info!(
                                        "wlan_fix: WlanQueryInterface hooked (attempt {})",
                                        attempt
                                    );
                                    return;
                                }
                                Err(err) => warn!("wlan_fix: enable failed: {err}"),
                            },
                            Err(err) => warn!("wlan_fix: initialize failed: {err}"),
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(200));
        }
        warn!("wlan_fix: gave up hooking WlanQueryInterface");
    });
}
