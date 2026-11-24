use serde::{Deserialize, Serialize};

use super::{mvs::Device, set_enum_verified};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RatioUnit {
    DeviceInteger,
    Ratio,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhiteBalance {
    pub unit: RatioUnit,
    pub red: f64,
    pub green: f64,
    pub blue: f64,
}

impl WhiteBalance {
    pub(crate) fn validate(&self) -> Result<(), String> {
        for value in [self.red, self.green, self.blue] {
            if !value.is_finite() || value <= 0.0 {
                return Err("白平衡通道增益须为正有限数".into());
            }
            match self.unit {
                RatioUnit::DeviceInteger if value.fract() != 0.0 || value > u32::MAX as f64 => {
                    return Err("设备整数刻度的白平衡增益须为正整数".into());
                }
                RatioUnit::Ratio if !(value as f32).is_finite() || value as f32 == 0.0 => {
                    return Err("白平衡倍率超出设备浮点范围".into());
                }
                _ => {}
            }
        }
        Ok(())
    }
}

pub(super) fn lock_and_read(
    device: &Device,
    requested: Option<WhiteBalance>,
) -> Result<WhiteBalance, String> {
    let (node, kind) = match device.node_type("BalanceRatio")? {
        Some(kind) => ("BalanceRatio", kind),
        None => (
            "BalanceRatioAbs",
            device
                .node_type("BalanceRatioAbs")?
                .ok_or("缺少白平衡倍率节点")?,
        ),
    };
    let unit = match kind {
        2 => RatioUnit::DeviceInteger,
        5 => RatioUnit::Ratio,
        _ => return Err(format!("{node} 节点类型 {kind} 不支持白平衡读写")),
    };
    if let Some(target) = requested {
        target.validate()?;
        if target.unit != unit {
            return Err("保存的白平衡刻度与当前相机不同，请重新取用相机回读值".into());
        }
    }
    if device.get_enum("BalanceWhiteAuto")? != "Off" {
        set_enum_verified(device, "BalanceWhiteAuto", "Off")?;
    }
    let read = || match unit {
        RatioUnit::DeviceInteger => device.get_int(node).map(|v| v as f64),
        RatioUnit::Ratio => device.get_float(node).map(f64::from),
    };
    let matches = |actual: f64, expected: f64| match unit {
        RatioUnit::DeviceInteger => actual == expected,
        RatioUnit::Ratio => (actual - expected).abs() <= expected * f64::from(f32::EPSILON) * 4.0,
    };
    if let Some(target) = requested {
        for (selector, expected) in [
            ("Red", target.red),
            ("Green", target.green),
            ("Blue", target.blue),
        ] {
            set_enum_verified(device, "BalanceRatioSelector", selector)?;
            if !matches(read()?, expected) {
                match unit {
                    RatioUnit::DeviceInteger => device.set_int(node, expected as i64)?,
                    RatioUnit::Ratio => device.set_float(node, expected as f32)?,
                }
            }
        }
    }
    let read_channel = |selector: &str, target: Option<f64>| -> Result<f64, String> {
        set_enum_verified(device, "BalanceRatioSelector", selector)?;
        let actual = read()?;
        if let Some(expected) = target {
            if !matches(actual, expected) {
                return Err(format!("{selector} 白平衡回读 {actual}，期望 {expected}"));
            }
        }
        Ok(actual)
    };
    let actual = WhiteBalance {
        unit,
        red: read_channel("Red", requested.map(|v| v.red))?,
        green: read_channel("Green", requested.map(|v| v.green))?,
        blue: read_channel("Blue", requested.map(|v| v.blue))?,
    };
    actual.validate()?;
    Ok(actual)
}
