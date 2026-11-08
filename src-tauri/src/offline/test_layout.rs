use super::s7_server::{BitAddr, Memory, SharedMemory};
use crate::handshake::{Contract, PROTOCOL_VERSION};
use crate::inspection::tag;
use ly_plc::{DataType, PlcConfig};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

const FIELDS: &[&str] = &[
    tag::PLC_VERSION,
    tag::PLC_HEARTBEAT,
    tag::PLC_STATE,
    tag::PLC_REASON,
    tag::REQUEST_SEQ,
    tag::PRODUCT_CODE,
    tag::SN,
    tag::PC_VERSION,
    tag::PC_HEARTBEAT,
    tag::PC_STATE,
    tag::PC_MODE,
    tag::SEQ_ECHO,
    tag::RESULT_CODE,
    tag::FAULT_CODE,
    tag::SN_ECHO,
];

#[derive(Clone, Copy)]
struct Slot {
    db: u16,
    byte: u32,
    bit: Option<u8>,
    len: u32,
    /// 字符串字段（S7 STRING）的头字节数：声明长度 + 实际长度
    text_header: Option<usize>,
}

/// 握手字段在 DB 中的位置，全部从 PLC 配置的握手契约解析。
pub struct Layout {
    slots: HashMap<&'static str, Slot>,
}

impl Layout {
    pub fn from_config(config: &PlcConfig) -> Result<Self, String> {
        let contract = Contract::validate(config)?;
        let mut slots = HashMap::new();
        for &field in FIELDS {
            let id = contract.point_id(field)?;
            let point = config
                .points
                .iter()
                .find(|p| p.id == id)
                .ok_or_else(|| format!("握手点位 {id} 不存在"))?;
            let location = ly_plc::resolve(&config.connection, &point.address, point.data_type)?;
            let text_header = (point.data_type == DataType::String).then_some(2);
            slots.insert(
                field,
                Slot {
                    db: location.area.index,
                    byte: location.start,
                    bit: location.bit,
                    len: location.len,
                    text_header,
                },
            );
        }
        Ok(Self { slots })
    }

    /// 心跳 Int 低字节的最低位：翻转后 plcHeartbeat 在 0/1 间交替。
    pub fn heartbeat(&self) -> BitAddr {
        let slot = self.slots[tag::PLC_HEARTBEAT];
        BitAddr {
            db: slot.db,
            byte: slot.byte + slot.len - 1,
            bit: 0,
        }
    }

    /// 按点表分配 DB（至少 1024 字节），PLC 协议版本预置为当前版本，字符串字段预置为空串（S7 STRING 带正确的声明长度）。
    pub fn memory(&self) -> SharedMemory {
        let mut extents = BTreeMap::<u16, usize>::new();
        for slot in self.slots.values() {
            let end = (slot.byte + slot.len) as usize;
            extents
                .entry(slot.db)
                .and_modify(|e| *e = (*e).max(end))
                .or_insert(end);
        }
        let mut memory = Memory::default();
        for (db, end) in extents {
            memory.create(db, end.max(1024));
        }
        self.set(&mut memory, tag::PLC_VERSION, PROTOCOL_VERSION as u32);
        for (field, slot) in &self.slots {
            if slot.text_header.is_some() {
                self.set_text(&mut memory, field, "");
            }
        }
        Arc::new(Mutex::new(memory))
    }

    pub(crate) fn get(&self, memory: &Memory, field: &str) -> u32 {
        let slot = self.slots[field];
        match slot.bit {
            Some(bit) => memory.bit(slot.db, slot.byte, bit).unwrap_or(false) as u32,
            None => memory
                .read(slot.db, slot.byte, slot.len as usize)
                .map(|bytes| {
                    bytes
                        .iter()
                        .fold(0u32, |value, byte| (value << 8) | *byte as u32)
                })
                .unwrap_or(0),
        }
    }

    pub(crate) fn get_text(&self, memory: &Memory, field: &str) -> String {
        let slot = self.slots[field];
        let header = slot.text_header.unwrap_or(2);
        memory
            .read(slot.db, slot.byte, slot.len as usize)
            .ok()
            .and_then(|bytes| {
                bytes
                    .get(header..header + bytes[header - 1] as usize)
                    .map(|chars| chars.iter().map(|&b| b as char).collect())
            })
            .unwrap_or_default()
    }

    pub(crate) fn set_text(&self, memory: &mut Memory, field: &str, value: &str) {
        let slot = self.slots[field];
        let header = slot.text_header.unwrap_or(2);
        let mut bytes = vec![0u8; slot.len as usize];
        if header == 2 {
            bytes[0] = (slot.len - 2) as u8;
        }
        bytes[header - 1] = value.len() as u8;
        bytes[header..header + value.len()].copy_from_slice(value.as_bytes());
        let _ = memory.write(slot.db, slot.byte, &bytes);
    }

    pub(crate) fn set(&self, memory: &mut Memory, field: &str, value: u32) {
        let slot = self.slots[field];
        let _ = match slot.bit {
            Some(bit) => memory.set_bit(slot.db, slot.byte, bit, value != 0),
            None => memory.write(
                slot.db,
                slot.byte,
                &value.to_be_bytes()[4 - slot.len as usize..],
            ),
        };
    }
}
