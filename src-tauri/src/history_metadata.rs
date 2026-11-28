use std::borrow::Cow;
use std::fmt;

use serde::de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

pub(crate) fn part_metadata(bytes: &[u8]) -> serde_json::Result<Value> {
    decode(bytes, Projection::Part)
}

pub(crate) fn capture_round(bytes: &[u8]) -> serde_json::Result<Value> {
    decode(bytes, Projection::Round)
}

fn decode(bytes: &[u8], projection: Projection) -> serde_json::Result<Value> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let projected = projection
        .deserialize(&mut decoder)
        .and_then(|value| decoder.end().map(|()| value));
    // 完整解码保留 serde_json 的保留对象键语义和原有报错位置。
    projected.or_else(|_| serde_json::from_slice(bytes))
}

#[derive(Deserialize)]
#[serde(transparent)]
struct Key<'a>(#[serde(borrow)] Cow<'a, str>);

enum Projection {
    Part,
    Round,
    Frames,
    Frame,
    Discard,
}

impl<'de> DeserializeSeed<'de> for Projection {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Value, D::Error> {
        decoder.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Projection {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JSON value")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut fields = Map::new();
        while let Some(Key(key)) = map.next_key::<Key<'de>>()? {
            if matches!(
                key.as_ref(),
                "$serde_json::private::RawValue" | "$serde_json::private::Number"
            ) {
                return Err(A::Error::custom("reserved JSON object key"));
            }
            let value = match (&self, key.as_ref()) {
                (Self::Part, "frames") => map.next_value_seed(Self::Frames)?,
                (
                    Self::Part,
                    "cycleId" | "fixture" | "cameras" | "groupCount" | "missingFrames"
                    | "rawDropped" | "devices" | "plc",
                )
                | (
                    Self::Frame,
                    "camera" | "seq" | "ts" | "available" | "manual" | "file" | "cameraId"
                    | "frameCounter" | "width" | "height" | "session",
                )
                | (Self::Round, "origin") => map.next_value()?,
                _ => {
                    map.next_value_seed(Self::Discard)?;
                    continue;
                }
            };
            fields.insert(key.into_owned(), value);
        }
        Ok(Value::Object(fields))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        if matches!(self, Self::Frames) {
            let mut frames = Vec::new();
            while let Some(frame) = seq.next_element_seed(Self::Frame)? {
                frames.push(frame);
            }
            Ok(Value::Array(frames))
        } else {
            while seq.next_element_seed(Self::Discard)?.is_some() {}
            Ok(Value::Null)
        }
    }

    fn visit_bool<E>(self, _: bool) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_str<E>(self, _: &str) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
}
