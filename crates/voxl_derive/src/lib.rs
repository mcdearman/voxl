//! `#[derive(Component)]` and `#[derive(Reflect)]` for voxl.
//!
//! The generated code names the engine as `::voxl`, which also works inside the engine
//! itself (it declares `extern crate self as voxl`).

use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput, Fields, LitStr};

/// Marks a type as something that can be attached to entities.
#[proc_macro_derive(Component)]
pub fn derive_component(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    quote! {
        impl #impl_generics ::voxl::ecs::Component for #name #type_generics #where_clause {}
    }
    .into()
}

/// What `#[reflect(...)]` says about a type or a field.
#[derive(Default)]
struct Options {
    /// The name the type goes by in saved files, when not its Rust name.
    name: Option<String>,
    /// On a type: fields missing from saved data come from `Default::default()`.
    /// On a field: this field does, if it is missing.
    default: bool,
    /// On a field: never saved; made with `Default::default()` when loading.
    skip: bool,
}

fn options(attrs: &[syn::Attribute]) -> syn::Result<Options> {
    let mut options = Options::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident("reflect")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                options.name = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("default") {
                options.default = true;
            } else if meta.path.is_ident("skip") {
                options.skip = true;
            } else {
                return Err(meta.error("expected `name = \"...\"`, `default` or `skip`"));
            }
            Ok(())
        })?;
    }
    Ok(options)
}

/// Makes a type describable, savable and loadable at runtime.
///
/// A struct with named fields becomes a map, a tuple struct a list (a single field is saved
/// as just that field), and an enum is saved as its variant's name, with its fields if it has
/// any. Every field's type must itself be `Reflect`.
///
/// - `#[reflect(name = "voxl.Transform")]` on the type: the name it has in saved files.
/// - `#[reflect(default)]` on the type (which must be `Default`): missing fields are filled
///   from the default value. On a field: that field is, from its own type's default.
/// - `#[reflect(skip)]` on a field: it is not saved, and is made with `Default::default()`.
#[proc_macro_derive(Reflect, attributes(reflect))]
pub fn derive_reflect(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_reflect(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Builds the code that makes a type's fields from a `&Value` called `value`, given an
/// expression for a default value of the whole type if there is one.
type Load = Box<dyn Fn(Option<&Tokens>) -> Tokens>;

/// How to save, load and describe one set of fields (a struct's, or one enum variant's).
struct Shape {
    /// A pattern binding every field, to follow the type or variant path.
    pattern: Tokens,
    /// Builds the `Value` from those bindings.
    to_value: Tokens,
    /// Builds the fields from a `&Value` called `value`, to follow the type or variant path.
    /// `fallback` is an expression for a default value of the whole type, if there is one.
    from_value: Load,
    schema: Tokens,
}

fn shape(fields: &Fields, what: &str) -> syn::Result<Shape> {
    match fields {
        Fields::Unit => Ok(Shape {
            pattern: quote!(),
            to_value: quote!(::voxl::reflect::Value::Null),
            from_value: Box::new(|_| quote!()),
            schema: quote!(::voxl::reflect::Schema::Unit),
        }),
        Fields::Named(named) => {
            let mut idents = Vec::new();
            let mut saved = Vec::new();
            let mut loads = Vec::new();
            let mut schemas = Vec::new();
            for field in &named.named {
                let ident = field.ident.clone().unwrap();
                let key = ident.to_string();
                let ty = &field.ty;
                let options = options(&field.attrs)?;
                if options.skip {
                    // Bound to nothing when saving, so it isn't an unused variable.
                    idents.push(quote!(#ident: _));
                    loads.push((ident, quote!(::core::default::Default::default()), None));
                    continue;
                }
                idents.push(quote!(#ident));
                saved.push(quote! {
                    (::std::string::String::from(#key), ::voxl::reflect::Reflect::to_value(#ident))
                });
                schemas.push(quote!((#key, <#ty as ::voxl::reflect::Reflect>::schema())));
                let missing = if options.default {
                    quote!(::core::default::Default::default())
                } else {
                    quote!(return ::core::result::Result::Err(
                        ::voxl::reflect::ReflectError::missing(#what, #key)
                    ))
                };
                let present = quote! {
                    <#ty as ::voxl::reflect::Reflect>::from_value(field)
                        .map_err(|err| err.inside(#key))?
                };
                loads.push((ident, present, Some((key, missing))));
            }
            let from_value = move |fallback: Option<&Tokens>| {
                // Checked once, where the first saved field is read: without it a value of
                // the wrong shape would have every field missing, and load as the default.
                let mut check = Some(quote! {
                    if !::core::matches!(value, ::voxl::reflect::Value::Map(_)) {
                        return ::core::result::Result::Err(
                            ::voxl::reflect::ReflectError::expected("a map", value)
                        );
                    }
                });
                let fields = loads.iter().map(|(ident, present, lookup)| match lookup {
                    None => quote!(#ident: #present),
                    Some((key, missing)) => {
                        let check = check.take();
                        // With a default for the whole type, a missing field takes its value
                        // from there instead of being an error.
                        let missing = match fallback {
                            Some(_) => quote!(fallback.#ident),
                            None => missing.clone(),
                        };
                        quote! {
                            #ident: {
                                #check
                                match value.field(#key) {
                                    ::core::option::Option::Some(field) => #present,
                                    ::core::option::Option::None => #missing,
                                }
                            }
                        }
                    }
                });
                quote!({ #(#fields),* })
            };
            Ok(Shape {
                pattern: quote!({ #(#idents),* }),
                to_value: quote!(::voxl::reflect::Value::Map(::std::vec![#(#saved),*])),
                from_value: Box::new(from_value),
                schema: quote!(::voxl::reflect::Schema::Fields(::std::vec![#(#schemas),*])),
            })
        }
        Fields::Unnamed(unnamed) => {
            let count = unnamed.unnamed.len();
            let bindings: Vec<_> = (0..count).map(|i| format_ident!("field_{i}")).collect();
            let types: Vec<_> = unnamed.unnamed.iter().map(|f| f.ty.clone()).collect();
            for field in &unnamed.unnamed {
                let options = options(&field.attrs)?;
                if options.skip || options.default {
                    return Err(syn::Error::new_spanned(
                        field,
                        "`skip` and `default` are for named fields",
                    ));
                }
            }
            if count == 1 {
                // A wrapper is saved as what it wraps.
                let ty = types[0].clone();
                let binding = &bindings[0];
                let schema = quote!(<#ty as ::voxl::reflect::Reflect>::schema());
                return Ok(Shape {
                    pattern: quote!((#binding)),
                    to_value: quote!(::voxl::reflect::Reflect::to_value(#binding)),
                    from_value: Box::new(
                        move |_| quote!((<#ty as ::voxl::reflect::Reflect>::from_value(value)?)),
                    ),
                    schema,
                });
            }
            let indices: Vec<_> = (0..count).collect();
            let what = what.to_owned();
            let load_types = types.clone();
            Ok(Shape {
                pattern: quote!((#(#bindings),*)),
                to_value: quote! {
                    ::voxl::reflect::Value::List(::std::vec![
                        #(::voxl::reflect::Reflect::to_value(#bindings)),*
                    ])
                },
                from_value: Box::new(move |_| {
                    quote! {
                        (#(
                            <#load_types as ::voxl::reflect::Reflect>::from_value(
                                value.item(#indices).ok_or_else(|| {
                                    ::voxl::reflect::ReflectError::expected(#what, value)
                                })?
                            ).map_err(|err| err.inside_index(#indices))?
                        ),*)
                    }
                }),
                schema: quote! {
                    ::voxl::reflect::Schema::Tuple(::std::vec![
                        #(<#types as ::voxl::reflect::Reflect>::schema()),*
                    ])
                },
            })
        }
    }
}

fn expand_reflect(input: &DeriveInput) -> syn::Result<Tokens> {
    let ident = &input.ident;
    let options = options(&input.attrs)?;
    let type_name = options.name.clone().unwrap_or_else(|| ident.to_string());
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    let fallback = options
        .default
        .then(|| quote!(<Self as ::core::default::Default>::default()));

    let (to_value, from_value, schema) = match &input.data {
        Data::Struct(data) => {
            let shape = shape(&data.fields, &type_name)?;
            let (pattern, to_value, schema) = (&shape.pattern, &shape.to_value, &shape.schema);
            let build = (shape.from_value)(fallback.as_ref());
            let bind_fallback = fallback
                .as_ref()
                .map(|default| quote!(let fallback = #default;));
            (
                quote! {
                    let Self #pattern = self;
                    #to_value
                },
                quote! {
                    #bind_fallback
                    ::core::result::Result::Ok(Self #build)
                },
                quote! {
                    ::voxl::reflect::Schema::Struct {
                        name: #type_name,
                        fields: ::std::boxed::Box::new(#schema),
                    }
                },
            )
        }
        Data::Enum(data) => {
            let mut saves = Vec::new();
            let mut loads = Vec::new();
            let mut schemas = Vec::new();
            for variant in &data.variants {
                let variant_ident = &variant.ident;
                let variant_name = variant_ident.to_string();
                let shape = shape(&variant.fields, &variant_name)?;
                let (pattern, to_value, schema) = (&shape.pattern, &shape.to_value, &shape.schema);
                let build = (shape.from_value)(None);
                schemas.push(quote!((#variant_name, #schema)));
                if matches!(variant.fields, Fields::Unit) {
                    saves.push(quote! {
                        Self::#variant_ident => ::voxl::reflect::Value::Text(
                            ::std::string::String::from(#variant_name)
                        )
                    });
                    loads.push(quote!((#variant_name, _) => ::core::result::Result::Ok(Self::#variant_ident)));
                } else {
                    saves.push(quote! {
                        Self::#variant_ident #pattern => ::voxl::reflect::Value::Map(::std::vec![(
                            ::std::string::String::from(#variant_name),
                            #to_value,
                        )])
                    });
                    loads.push(quote! {
                        (#variant_name, ::core::option::Option::Some(value)) => {
                            ::core::result::Result::Ok(Self::#variant_ident #build)
                        }
                    });
                }
            }
            (
                quote! {
                    match self {
                        #(#saves),*
                    }
                },
                quote! {
                    match value.variant() {
                        ::core::option::Option::Some(tagged) => match tagged {
                            #(#loads,)*
                            _ => ::core::result::Result::Err(
                                ::voxl::reflect::ReflectError::expected(#type_name, value)
                            ),
                        },
                        ::core::option::Option::None => ::core::result::Result::Err(
                            ::voxl::reflect::ReflectError::expected(#type_name, value)
                        ),
                    }
                },
                quote! {
                    ::voxl::reflect::Schema::Enum {
                        name: #type_name,
                        variants: ::std::vec![#(#schemas),*],
                    }
                },
            )
        }
        Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                ident,
                "Reflect can't be derived for unions",
            ))
        }
    };

    Ok(quote! {
        impl #impl_generics ::voxl::reflect::Reflect for #ident #type_generics #where_clause {
            fn type_name() -> &'static str {
                #type_name
            }

            fn to_value(&self) -> ::voxl::reflect::Value {
                #to_value
            }

            #[allow(unused_variables)]
            fn from_value(
                value: &::voxl::reflect::Value,
            ) -> ::core::result::Result<Self, ::voxl::reflect::ReflectError> {
                #from_value
            }

            fn schema() -> ::voxl::reflect::Schema {
                #schema
            }
        }
    })
}
