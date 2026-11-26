//! Reads YAML into `serde_yaml::Value` the way Home Assistant's loader does where it
//! differs from serde_yaml: a duplicate key is a warning and the last one wins.

use std::cell::RefCell;
use std::fmt;

use serde::de::{
    DeserializeSeed, Deserializer, EnumAccess, Error, MapAccess, SeqAccess, VariantAccess, Visitor,
};
use serde_yaml::value::{Tag, TaggedValue};
use serde_yaml::{Mapping, Value};

pub(super) struct Loose<'a> {
    pub duplicates: &'a RefCell<Vec<String>>,
}

impl<'de> DeserializeSeed<'de> for Loose<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Loose<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any YAML value")
    }

    fn visit_bool<E>(self, b: bool) -> Result<Value, E> {
        Ok(Value::Bool(b))
    }

    fn visit_i64<E>(self, i: i64) -> Result<Value, E> {
        Ok(Value::Number(i.into()))
    }

    fn visit_u64<E>(self, u: u64) -> Result<Value, E> {
        Ok(Value::Number(u.into()))
    }

    fn visit_f64<E>(self, f: f64) -> Result<Value, E> {
        Ok(Value::Number(f.into()))
    }

    fn visit_str<E>(self, s: &str) -> Result<Value, E> {
        Ok(Value::String(s.to_owned()))
    }

    fn visit_string<E>(self, s: String) -> Result<Value, E> {
        Ok(Value::String(s))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(self)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(v) = seq.next_element_seed(Loose { duplicates: self.duplicates })? {
            items.push(v);
        }
        Ok(Value::Sequence(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut m = Mapping::new();
        while let Some(k) = map.next_key_seed(Loose { duplicates: self.duplicates })? {
            let v = map.next_value_seed(Loose { duplicates: self.duplicates })?;
            if m.contains_key(&k) {
                let name = k.as_str().map_or_else(|| format!("{k:?}"), str::to_owned);
                self.duplicates.borrow_mut().push(name);
            }
            m.insert(k, v);
        }
        Ok(Value::Mapping(m))
    }

    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Value, A::Error> {
        let (tag, contents) = data.variant_seed(TagName)?;
        let value = contents.newtype_variant_seed(Loose { duplicates: self.duplicates })?;
        Ok(Value::Tagged(Box::new(TaggedValue { tag: Tag::new(tag), value })))
    }
}

/// serde_yaml hands a tagged node to `visit_enum` with the tag as the variant name.
struct TagName;

impl<'de> DeserializeSeed<'de> for TagName {
    type Value = String;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<String, D::Error> {
        d.deserialize_string(self)
    }
}

impl Visitor<'_> for TagName {
    type Value = String;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a YAML tag")
    }

    fn visit_str<E: Error>(self, s: &str) -> Result<String, E> {
        if s.is_empty() {
            return Err(E::custom("empty YAML tag is not allowed"));
        }
        Ok(s.to_owned())
    }
}
