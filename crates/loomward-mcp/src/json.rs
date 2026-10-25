//! Duplicate-key refusal at every depth, before method dispatch. `Value` alone loses duplicates.
use serde::{de, Deserialize, Deserializer};
use serde_json::{Map, Value};

pub fn parse(bytes: &[u8]) -> Result<Value, String> {
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let Strict(value) = Strict::deserialize(&mut de).map_err(|e| e.to_string())?;
    de.end().map_err(|e| e.to_string())?;
    fn depth_ok(v: &Value, depth: usize) -> bool {
        depth <= 32
            && match v {
                Value::Object(m) => m.values().all(|v| depth_ok(v, depth + 1)),
                Value::Array(a) => a.iter().all(|v| depth_ok(v, depth + 1)),
                _ => true,
            }
    }
    if !depth_ok(&value, 0) {
        return Err("depth limit".into());
    }
    Ok(value)
}

struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = Strict;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON value without duplicate members")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Strict, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| Strict(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Strict, A::Error> {
                let mut out = vec![];
                while let Some(Strict(v)) = seq.next_element()? {
                    out.push(v);
                }
                Ok(Strict(Value::Array(out)))
            }
            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Strict, A::Error> {
                let mut out = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if out.contains_key(&key) {
                        return Err(de::Error::custom("duplicate member"));
                    }
                    let Strict(value) = map.next_value()?;
                    out.insert(key, value);
                }
                Ok(Strict(Value::Object(out)))
            }
        }
        d.deserialize_any(Visitor)
    }
}
