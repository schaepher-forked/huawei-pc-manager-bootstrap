//! 生成「碰一碰」NFC 标签的二维码。
//!
//! 载荷格式与写标签流程参考：
//! - <https://github.com/Masterchiefm/Thirdparty-huawei-Share-OneHop>
//! - <https://github.com/difaandailisi/NFCSHARE-Honor-Edition>
//!
//! 本工具只负责「算数据 + 出二维码」，不直接写 NFC：
//! 用户用手机上的「一碰传助手」扫码，再把内容写入 NFC 标签即可。

use std::mem::{size_of, zeroed};

use anyhow::{bail, Context};
use iced::widget::qr_code;
use windows_sys::Win32::{
    Devices::Bluetooth::{
        BluetoothFindFirstRadio, BluetoothFindRadioClose, BluetoothGetRadioInfo,
        BLUETOOTH_FIND_RADIO_PARAMS, BLUETOOTH_RADIO_INFO,
    },
    Foundation::{CloseHandle, HANDLE},
};

use common::config::{get_firmware_config, Config};

/// 电脑在一碰传协同码表里的固定型号 ID。
pub const PC_MODEL_ID: &str = "00000506";
/// 一碰传载荷里的 SN 固定长度。
pub const SN_LENGTH: usize = 16;

/// 生成好的一份标签数据，`qr` 可以直接交给 iced 渲染。
pub struct NfcTag {
    pub sn: String,
    pub mac: String,
    pub payload: String,
    pub qr: qr_code::State,
}

impl NfcTag {
    /// 读取当前生效的 SN（来自补丁配置）和本机蓝牙 MAC，生成标签。
    pub fn generate() -> anyhow::Result<Self> {
        Self::from_parts(&effective_sn(), &get_bluetooth_mac()?)
    }

    pub fn from_parts(raw_sn: &str, raw_mac: &str) -> anyhow::Result<Self> {
        let sn = normalize_sn(raw_sn);
        let mac =
            normalize_mac(raw_mac).with_context(|| format!("蓝牙 MAC 格式无效：{:?}", raw_mac))?;
        let payload = build_payload(&sn, &mac);
        let qr = qr_code::State::new(&payload).context("生成二维码失败")?;

        Ok(Self {
            sn,
            mac,
            payload,
            qr,
        })
    }
}

/// 电脑管家实际看到的 SN：补丁会把 SMBIOS 的 SN 替换成配置里的值，
/// 因此标签必须用同一份 SN，否则扫码后无法与电脑管家匹配。
pub fn effective_sn() -> String {
    get_firmware_config()
        .unwrap_or_else(|_| Config::default())
        .sn
}

pub fn build_payload(sn: &str, mac: &str) -> String {
    format!("SN={sn}|MAC={mac}|MODELID={PC_MODEL_ID}")
}

/// 与参考实现保持一致：空格转 `A`，右补 `A` 到 16 位，超长截断。
pub fn normalize_sn(raw: &str) -> String {
    let mut sn: String = raw
        .trim()
        .chars()
        .map(|c| if c == ' ' { 'A' } else { c })
        .take(SN_LENGTH)
        .collect();
    while sn.chars().count() < SN_LENGTH {
        sn.push('A');
    }
    sn
}

/// 去掉 `:` / `-` 并转大写，必须是 12 位十六进制。
pub fn normalize_mac(raw: &str) -> Option<String> {
    let mac: String = raw
        .chars()
        .filter(|c| *c != ':' && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if mac.len() == 12 && mac.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(mac)
    } else {
        None
    }
}

/// 取本机蓝牙适配器的 MAC（BD_ADDR），格式为 12 位大写十六进制。
///
/// 注意 Windows 的 `ullLong` 已经是 `AABBCCDDEEFF` 这种可读顺序，
/// 按 12 位十六进制输出即可，不需要再反转字节。
pub fn get_bluetooth_mac() -> anyhow::Result<String> {
    unsafe {
        let params = BLUETOOTH_FIND_RADIO_PARAMS {
            dwSize: size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32,
        };
        let mut radio: HANDLE = 0;
        let find = BluetoothFindFirstRadio(&params, &mut radio);
        if find == 0 {
            bail!(
                "未找到蓝牙适配器，请确认蓝牙已开启（{}）",
                std::io::Error::last_os_error()
            );
        }

        let mut info: BLUETOOTH_RADIO_INFO = zeroed();
        info.dwSize = size_of::<BLUETOOTH_RADIO_INFO>() as u32;
        let result = BluetoothGetRadioInfo(radio, &mut info);

        CloseHandle(radio);
        BluetoothFindRadioClose(find);

        if result != 0 {
            bail!(
                "读取蓝牙 MAC 失败：{}",
                std::io::Error::from_raw_os_error(result as i32)
            );
        }

        Ok(format!("{:012X}", info.address.Anonymous.ullLong))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sn_is_padded_and_truncated() {
        assert_eq!(normalize_sn("ABC"), "ABCAAAAAAAAAAAAA");
        assert_eq!(normalize_sn("ABC DEF"), "ABCADEFAAAAAAAAA");
        assert_eq!(normalize_sn("12345678901234567890"), "1234567890123456");
        assert_eq!(normalize_sn("5EKPM18320000397"), "5EKPM18320000397");
    }

    #[test]
    fn mac_is_normalized() {
        assert_eq!(
            normalize_mac("AA:BB:CC:DD:EE:FF").as_deref(),
            Some("AABBCCDDEEFF")
        );
        assert_eq!(
            normalize_mac("AA-BB-CC-DD-EE-FF").as_deref(),
            Some("AABBCCDDEEFF")
        );
        assert_eq!(normalize_mac("AABBCCDDEE"), None);
        assert_eq!(normalize_mac("ZZ1A7DDA7113"), None);
    }

    #[test]
    fn payload_matches_reference_format() {
        let sn = normalize_sn("5EKPM18320000397");
        let mac = normalize_mac("AA:BB:CC:DD:EE:FF").unwrap();
        assert_eq!(
            build_payload(&sn, &mac),
            "SN=5EKPM18320000397|MAC=AABBCCDDEEFF|MODELID=00000506"
        );
    }
}
