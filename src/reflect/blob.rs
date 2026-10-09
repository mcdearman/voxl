//! Reflection for components defined at runtime, which have no Rust type: a native plugin
//! says what fields its component has, and from then on the component can be saved in scenes
//! and shown in an inspector like any other.

use std::alloc::Layout;

use super::{
    registry::{intern, ComponentType},
    value::{ReflectError, Schema, Value},
};
use crate::ecs::ComponentKey;

/// What one field of a runtime-defined component holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    F32,
    F64,
    I32,
    I64,
    U8,
    U32,
    /// One byte: zero is false.
    Bool,
    /// An entity handle, 64 bits. Scenes keep it pointing at the right entity.
    Entity,
}

impl FieldKind {
    fn size(self) -> usize {
        match self {
            FieldKind::U8 | FieldKind::Bool => 1,
            FieldKind::F32 | FieldKind::I32 | FieldKind::U32 => 4,
            FieldKind::F64 | FieldKind::I64 | FieldKind::Entity => 8,
        }
    }

    fn schema(self) -> Schema {
        match self {
            FieldKind::F32 | FieldKind::F64 => Schema::Float,
            FieldKind::Bool => Schema::Bool,
            FieldKind::Entity => Schema::Entity,
            _ => Schema::Int,
        }
    }

    /// # Safety
    /// `at` must be readable for this kind's size. It need not be aligned.
    unsafe fn read(self, at: *const u8) -> Value {
        match self {
            FieldKind::F32 => Value::Float(at.cast::<f32>().read_unaligned() as f64),
            FieldKind::F64 => Value::Float(at.cast::<f64>().read_unaligned()),
            FieldKind::I32 => Value::Int(at.cast::<i32>().read_unaligned() as i64),
            FieldKind::I64 => Value::Int(at.cast::<i64>().read_unaligned()),
            FieldKind::U8 => Value::Int(at.read() as i64),
            FieldKind::U32 => Value::Int(at.cast::<u32>().read_unaligned() as i64),
            FieldKind::Bool => Value::Bool(at.read() != 0),
            FieldKind::Entity => Value::Entity(at.cast::<u64>().read_unaligned()),
        }
    }

    /// # Safety
    /// `at` must be writable for this kind's size. It need not be aligned.
    unsafe fn write(self, at: *mut u8, value: &Value) -> Result<(), ReflectError> {
        let whole = |what: &str| match value {
            Value::Int(i) => Ok(*i),
            other => Err(ReflectError::expected(what, other)),
        };
        let fits = |i: i64, what: &str| ReflectError::new(format!("{i} doesn't fit in {what}"));
        match self {
            FieldKind::F32 | FieldKind::F64 => {
                let f = value
                    .as_f64()
                    .ok_or_else(|| ReflectError::expected("a number", value))?;
                if self == FieldKind::F32 {
                    at.cast::<f32>().write_unaligned(f as f32);
                } else {
                    at.cast::<f64>().write_unaligned(f);
                }
            }
            FieldKind::I32 => {
                let i = whole("a whole number")?;
                let v = i32::try_from(i).map_err(|_| fits(i, "32 bits"))?;
                at.cast::<i32>().write_unaligned(v);
            }
            FieldKind::I64 => at.cast::<i64>().write_unaligned(whole("a whole number")?),
            FieldKind::U8 => {
                let i = whole("a whole number")?;
                at.write(u8::try_from(i).map_err(|_| fits(i, "a byte"))?);
            }
            FieldKind::U32 => {
                let i = whole("a whole number")?;
                let v = u32::try_from(i).map_err(|_| fits(i, "32 unsigned bits"))?;
                at.cast::<u32>().write_unaligned(v);
            }
            FieldKind::Bool => match value {
                Value::Bool(b) => at.write(*b as u8),
                other => return Err(ReflectError::expected("a boolean", other)),
            },
            FieldKind::Entity => match value {
                Value::Entity(bits) => at.cast::<u64>().write_unaligned(*bits),
                other => return Err(ReflectError::expected("an entity", other)),
            },
        }
        Ok(())
    }
}

/// One field of a runtime-defined component: `count` values of one kind (more than one for a
/// vector or an array) starting `offset` bytes in.
#[derive(Clone, Debug, PartialEq)]
pub struct BlobField {
    pub name: String,
    pub kind: FieldKind,
    pub count: usize,
    pub offset: usize,
}

/// Makes a runtime-defined component reachable through the type registry, given what its
/// fields are. Bytes no field covers are saved as nothing and loaded as zero.
pub fn blob_component_type(
    name: &str,
    key: ComponentKey,
    layout: Layout,
    fields: Vec<BlobField>,
) -> Result<ComponentType, String> {
    for (index, field) in fields.iter().enumerate() {
        let end = field
            .kind
            .size()
            .checked_mul(field.count)
            .and_then(|size| size.checked_add(field.offset));
        if field.count == 0 || end.is_none_or(|end| end > layout.size()) {
            return Err(format!(
                "field `{}` doesn't fit inside `{name}` ({} bytes)",
                field.name,
                layout.size()
            ));
        }
        if fields[..index].iter().any(|other| other.name == field.name) {
            return Err(format!("`{name}` has two fields called `{}`", field.name));
        }
    }

    let type_name = intern(name);
    let schema = Schema::Struct {
        name: type_name,
        fields: Box::new(Schema::Fields(
            fields
                .iter()
                .map(|field| {
                    let one = field.kind.schema();
                    let schema = if field.count == 1 {
                        one
                    } else {
                        Schema::Array(Box::new(one), field.count)
                    };
                    (intern(&field.name), schema)
                })
                .collect(),
        )),
    };
    let (read_fields, write_fields) = (fields.clone(), fields);

    Ok(ComponentType {
        name: type_name,
        schema: Box::new(move || schema.clone()),
        get: Box::new(move |world, entity| {
            let storage = world.erased_storage(key)?;
            // SAFETY: shared access to the world, and every field was checked to lie inside
            // the component.
            unsafe {
                let base = storage.value_ptr(entity)?.cast_const();
                let fields = read_fields.iter().map(|field| {
                    let at = |i: usize| base.add(field.offset + i * field.kind.size());
                    let value = if field.count == 1 {
                        field.kind.read(at(0))
                    } else {
                        Value::List((0..field.count).map(|i| field.kind.read(at(i))).collect())
                    };
                    (field.name.clone(), value)
                });
                Some(Value::Map(fields.collect()))
            }
        }),
        insert: Box::new(move |world, entity, value| {
            // Start from zeros, so anything the value leaves out is zero.
            let mut bytes = vec![0u8; layout.size()];
            for field in &write_fields {
                let Some(saved) = value.field(&field.name) else {
                    continue;
                };
                let inside = |err: ReflectError| err.inside(&field.name);
                // SAFETY: every field was checked to lie inside the component.
                unsafe {
                    let base = bytes.as_mut_ptr();
                    let at = |i: usize| base.add(field.offset + i * field.kind.size());
                    if field.count == 1 {
                        field.kind.write(at(0), saved).map_err(inside)?;
                    } else {
                        for i in 0..field.count {
                            let item = saved.item(i).ok_or_else(|| {
                                inside(ReflectError::new(format!("expected {} items", field.count)))
                            })?;
                            field
                                .kind
                                .write(at(i), item)
                                .map_err(|err| inside(err.inside_index(i)))?;
                        }
                    }
                }
            }
            // SAFETY: the bytes are a value of the component as its plugin described it.
            if unsafe { world.insert_raw(entity, key, bytes.as_ptr()) } {
                Ok(())
            } else {
                Err(ReflectError::new(
                    "the entity or the component no longer exists",
                ))
            }
        }),
        remove: Box::new(move |world, entity| world.remove_by_key(entity, key)),
        entities: Box::new(move |world| {
            world
                .erased_storage(key)
                .map_or(Vec::new(), |storage| storage.entities().to_vec())
        }),
    })
}
