use std::fmt;

use crate::ecs::Entity;

/// A value of any reflected type, taken apart into plain data: what gets written to a scene
/// file, shown in an inspector, or handed to code that doesn't know the Rust type.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    List(Vec<Value>),
    /// Named fields, in order.
    Map(Vec<(String, Value)>),
    /// A reference to an entity. Kept apart from plain numbers so that loading a scene can
    /// point it at the entity's new id.
    Entity(u64),
    /// A reference to an asset by name: what kind it is (`"image"`, `"mesh"`) and the name
    /// the `AssetServer` knows it by. Loading a scene loads the asset and puts its handle
    /// here.
    Asset {
        kind: String,
        name: String,
    },
}

impl Value {
    /// A named field of a map.
    pub fn field(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Map(fields) => fields.iter().find(|(key, _)| key == name).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn field_mut(&mut self, name: &str) -> Option<&mut Value> {
        match self {
            Value::Map(fields) => fields
                .iter_mut()
                .find(|(key, _)| key == name)
                .map(|(_, v)| v),
            _ => None,
        }
    }

    /// An item of a list.
    pub fn item(&self, index: usize) -> Option<&Value> {
        match self {
            Value::List(items) => items.get(index),
            _ => None,
        }
    }

    /// Reads this as an enum: a bare name is a variant without fields, and a map with one
    /// entry is a variant with them.
    pub fn variant(&self) -> Option<(&str, Option<&Value>)> {
        match self {
            Value::Text(name) => Some((name, None)),
            Value::Map(fields) if fields.len() == 1 => Some((&fields[0].0, Some(&fields[0].1))),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// Follows a path such as `translation.0` or `shape.Box.half`: names pick map fields and
    /// numbers pick list items.
    pub fn get_path(&self, path: &str) -> Option<&Value> {
        path.split('.')
            .filter(|part| !part.is_empty())
            .try_fold(self, |value, part| match part.parse::<usize>() {
                Ok(index) if matches!(value, Value::List(_)) => value.item(index),
                _ => value.field(part),
            })
    }

    /// Replaces what is at a path. Returns false if the path leads nowhere.
    pub fn set_path(&mut self, path: &str, new: Value) -> bool {
        let mut value = self;
        for part in path.split('.').filter(|part| !part.is_empty()) {
            let next = match (part.parse::<usize>(), value) {
                (Ok(index), Value::List(items)) => items.get_mut(index),
                (_, other) => other.field_mut(part),
            };
            match next {
                Some(next) => value = next,
                None => return false,
            }
        }
        *value = new;
        true
    }

    /// Calls `f` on every entity reference in this value, however deeply nested.
    pub fn for_each_entity(&mut self, f: &mut impl FnMut(&mut u64)) {
        match self {
            Value::Entity(bits) => f(bits),
            Value::List(items) => items.iter_mut().for_each(|item| item.for_each_entity(f)),
            Value::Map(fields) => fields.iter_mut().for_each(|(_, v)| v.for_each_entity(f)),
            _ => {}
        }
    }

    /// Calls `f` on every asset reference in this value, however deeply nested. `f` may
    /// replace it (with the loaded asset's id).
    pub fn for_each_asset(&mut self, f: &mut impl FnMut(&mut Value)) {
        match self {
            Value::Asset { .. } => f(self),
            Value::List(items) => items.iter_mut().for_each(|item| item.for_each_asset(f)),
            Value::Map(fields) => fields.iter_mut().for_each(|(_, v)| v.for_each_asset(f)),
            _ => {}
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "a boolean",
            Value::Int(_) | Value::Float(_) => "a number",
            Value::Text(_) => "text",
            Value::List(_) => "a list",
            Value::Map(_) => "a map",
            Value::Entity(_) => "an entity",
            Value::Asset { .. } => "an asset",
        }
    }
}

/// Why a value couldn't be turned into a Rust type.
#[derive(Clone, Debug, PartialEq)]
pub struct ReflectError {
    /// Where in the value the problem is, outermost first.
    path: Vec<String>,
    message: String,
}

impl ReflectError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            path: Vec::new(),
            message: message.into(),
        }
    }

    pub fn expected(what: &str, found: &Value) -> Self {
        Self::new(format!("expected {what}, found {}", found.kind()))
    }

    pub fn missing(owner: &str, field: &str) -> Self {
        Self::new(format!("{owner} has no `{field}`"))
    }

    /// Notes that the problem is inside the named field.
    pub fn inside(mut self, field: &str) -> Self {
        self.path.insert(0, field.to_owned());
        self
    }

    pub fn inside_index(self, index: usize) -> Self {
        self.inside(&index.to_string())
    }
}

impl fmt::Display for ReflectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "at `{}`: {}", self.path.join("."), self.message)
        }
    }
}

impl std::error::Error for ReflectError {}

/// The shape of a reflected type: what an inspector needs to lay out its fields, and what a
/// binding generator needs to describe it to another language.
#[derive(Clone, Debug, PartialEq)]
pub enum Schema {
    /// Any value at all: a field that holds a value of some other type (an override, say).
    Any,
    Unit,
    Bool,
    Int,
    Float,
    Text,
    Entity,
    List(Box<Schema>),
    /// Exactly this many of one type (vectors, matrices, arrays).
    Array(Box<Schema>, usize),
    Optional(Box<Schema>),
    Tuple(Vec<Schema>),
    /// Named fields, as inside a struct or an enum variant.
    Fields(Vec<(&'static str, Schema)>),
    Struct {
        name: &'static str,
        fields: Box<Schema>,
    },
    Enum {
        name: &'static str,
        variants: Vec<(&'static str, Schema)>,
    },
}

/// A type that can be taken apart into a [`Value`] and put back together, and can describe
/// its own shape. Derive it; implement it by hand only for types with a natural plain form
/// (a vector as three numbers).
pub trait Reflect: Sized + 'static {
    /// The name this type goes by in saved files.
    fn type_name() -> &'static str;
    fn to_value(&self) -> Value;
    fn from_value(value: &Value) -> Result<Self, ReflectError>;
    fn schema() -> Schema;
}

/// A value held as it is: for fields that carry a value of some other type.
impl Reflect for Value {
    fn type_name() -> &'static str {
        "Value"
    }

    fn to_value(&self) -> Value {
        self.clone()
    }

    fn from_value(value: &Value) -> Result<Self, ReflectError> {
        Ok(value.clone())
    }

    fn schema() -> Schema {
        Schema::Any
    }
}

impl Reflect for bool {
    fn type_name() -> &'static str {
        "bool"
    }

    fn to_value(&self) -> Value {
        Value::Bool(*self)
    }

    fn from_value(value: &Value) -> Result<Self, ReflectError> {
        match value {
            Value::Bool(b) => Ok(*b),
            other => Err(ReflectError::expected("a boolean", other)),
        }
    }

    fn schema() -> Schema {
        Schema::Bool
    }
}

macro_rules! reflect_int {
    ($($t:ty),*) => {$(
        impl Reflect for $t {
            fn type_name() -> &'static str {
                stringify!($t)
            }

            fn to_value(&self) -> Value {
                Value::Int(*self as i64)
            }

            fn from_value(value: &Value) -> Result<Self, ReflectError> {
                match value {
                    Value::Int(i) => <$t>::try_from(*i).map_err(|_| {
                        ReflectError::new(format!("{i} doesn't fit in {}", stringify!($t)))
                    }),
                    other => Err(ReflectError::expected("a whole number", other)),
                }
            }

            fn schema() -> Schema {
                Schema::Int
            }
        }
    )*};
}
reflect_int!(u8, u16, u32, i8, i16, i32, i64, usize);

macro_rules! reflect_float {
    ($($t:ty),*) => {$(
        impl Reflect for $t {
            fn type_name() -> &'static str {
                stringify!($t)
            }

            fn to_value(&self) -> Value {
                Value::Float(*self as f64)
            }

            fn from_value(value: &Value) -> Result<Self, ReflectError> {
                value
                    .as_f64()
                    .map(|f| f as $t)
                    .ok_or_else(|| ReflectError::expected("a number", value))
            }

            fn schema() -> Schema {
                Schema::Float
            }
        }
    )*};
}
reflect_float!(f32, f64);

impl Reflect for String {
    fn type_name() -> &'static str {
        "String"
    }

    fn to_value(&self) -> Value {
        Value::Text(self.clone())
    }

    fn from_value(value: &Value) -> Result<Self, ReflectError> {
        match value {
            Value::Text(text) => Ok(text.clone()),
            other => Err(ReflectError::expected("text", other)),
        }
    }

    fn schema() -> Schema {
        Schema::Text
    }
}

impl Reflect for Entity {
    fn type_name() -> &'static str {
        "Entity"
    }

    fn to_value(&self) -> Value {
        Value::Entity(self.to_bits())
    }

    fn from_value(value: &Value) -> Result<Self, ReflectError> {
        match value {
            Value::Entity(bits) => Ok(Entity::from_bits(*bits)),
            other => Err(ReflectError::expected("an entity", other)),
        }
    }

    fn schema() -> Schema {
        Schema::Entity
    }
}

impl<T: Reflect> Reflect for Option<T> {
    fn type_name() -> &'static str {
        "Option"
    }

    fn to_value(&self) -> Value {
        self.as_ref().map_or(Value::Null, T::to_value)
    }

    fn from_value(value: &Value) -> Result<Self, ReflectError> {
        match value {
            Value::Null => Ok(None),
            other => T::from_value(other).map(Some),
        }
    }

    fn schema() -> Schema {
        Schema::Optional(Box::new(T::schema()))
    }
}

impl<T: Reflect> Reflect for Vec<T> {
    fn type_name() -> &'static str {
        "Vec"
    }

    fn to_value(&self) -> Value {
        Value::List(self.iter().map(T::to_value).collect())
    }

    fn from_value(value: &Value) -> Result<Self, ReflectError> {
        match value {
            Value::List(items) => items
                .iter()
                .enumerate()
                .map(|(i, item)| T::from_value(item).map_err(|err| err.inside_index(i)))
                .collect(),
            other => Err(ReflectError::expected("a list", other)),
        }
    }

    fn schema() -> Schema {
        Schema::List(Box::new(T::schema()))
    }
}

impl<T: Reflect, const N: usize> Reflect for [T; N] {
    fn type_name() -> &'static str {
        "array"
    }

    fn to_value(&self) -> Value {
        Value::List(self.iter().map(T::to_value).collect())
    }

    fn from_value(value: &Value) -> Result<Self, ReflectError> {
        let items = Vec::<T>::from_value(value)?;
        let found = items.len();
        items
            .try_into()
            .map_err(|_| ReflectError::new(format!("expected {N} items, found {found}")))
    }

    fn schema() -> Schema {
        Schema::Array(Box::new(T::schema()), N)
    }
}

/// Tuples are saved as lists, one item per part.
macro_rules! reflect_tuple {
    ($(($($t:ident $i:tt),+);)*) => {$(
        impl<$($t: Reflect),+> Reflect for ($($t,)+) {
            fn type_name() -> &'static str {
                "tuple"
            }

            fn to_value(&self) -> Value {
                Value::List(vec![$(self.$i.to_value()),+])
            }

            fn from_value(value: &Value) -> Result<Self, ReflectError> {
                const COUNT: usize = [$($i),+].len();
                match value {
                    Value::List(items) if items.len() == COUNT => Ok(($(
                        $t::from_value(&items[$i]).map_err(|err| err.inside_index($i))?,
                    )+)),
                    Value::List(items) => Err(ReflectError::new(format!(
                        "expected {COUNT} items, found {}",
                        items.len()
                    ))),
                    other => Err(ReflectError::expected("a list", other)),
                }
            }

            fn schema() -> Schema {
                Schema::Tuple(vec![$($t::schema()),+])
            }
        }
    )*};
}
reflect_tuple! {
    (A 0, B 1);
    (A 0, B 1, C 2);
    (A 0, B 1, C 2, D 3);
}

/// Reflects a type as a fixed list of floats, through conversions to and from an array.
macro_rules! reflect_floats {
    ($($t:ty: $n:literal, $name:literal;)*) => {$(
        impl Reflect for $t {
            fn type_name() -> &'static str {
                $name
            }

            fn to_value(&self) -> Value {
                self.to_array().to_value()
            }

            fn from_value(value: &Value) -> Result<Self, ReflectError> {
                <[f32; $n]>::from_value(value).map(<$t>::from_array)
            }

            fn schema() -> Schema {
                Schema::Array(Box::new(Schema::Float), $n)
            }
        }
    )*};
}
reflect_floats! {
    glam::Vec2: 2, "Vec2";
    glam::Vec3: 3, "Vec3";
    glam::Vec4: 4, "Vec4";
    glam::Quat: 4, "Quat";
}

impl Reflect for glam::Mat4 {
    fn type_name() -> &'static str {
        "Mat4"
    }

    fn to_value(&self) -> Value {
        self.to_cols_array().to_value()
    }

    fn from_value(value: &Value) -> Result<Self, ReflectError> {
        <[f32; 16]>::from_value(value).map(|m| glam::Mat4::from_cols_array(&m))
    }

    fn schema() -> Schema {
        Schema::Array(Box::new(Schema::Float), 16)
    }
}
