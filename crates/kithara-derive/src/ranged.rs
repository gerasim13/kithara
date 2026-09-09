//! Expansion of `#[derive(Ranged)]`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    Data, DeriveInput, Error, Expr, Fields, Ident, Lit, Result, Type, UnOp, parse_macro_input,
    spanned::Spanned,
};

#[derive(Clone, Copy, PartialEq)]
enum Number {
    Float,
    Integer,
}

impl Number {
    fn of(ty: &Type) -> Option<(Self, &Ident)> {
        let Type::Path(path) = ty else { return None };
        let ident = &path.path.segments.last()?.ident;
        let number = match ident.to_string().as_str() {
            "f32" | "f64" => Some(Self::Float),
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64"
            | "u128" | "usize" => Some(Self::Integer),
            _ => None,
        }?;
        Some((number, ident))
    }
}

#[derive(PartialEq, PartialOrd)]
enum Value {
    Float(f64),
    Integer(bool, u128),
}

struct Bound {
    tokens: TokenStream2,
    value: Value,
}

impl Bound {
    fn read(expr: &Expr, number: Number) -> Result<Self> {
        let (literal, sign) = match expr {
            Expr::Unary(unary) if matches!(unary.op, UnOp::Neg(_)) => (&*unary.expr, -1.0),
            other => (other, 1.0),
        };
        let value = match (literal, number) {
            (Expr::Lit(literal), Number::Float) if let Lit::Float(value) = &literal.lit => {
                Value::Float(value.base10_parse::<f64>()? * sign)
            }
            (Expr::Lit(literal), Number::Integer) if let Lit::Int(value) = &literal.lit => {
                let magnitude = value.base10_parse::<u128>()?;
                let positive = sign > 0.0 || magnitude == 0;
                Value::Integer(
                    positive,
                    if positive {
                        magnitude
                    } else {
                        u128::MAX - magnitude
                    },
                )
            }
            _ => {
                return Err(Error::new(
                    expr.span(),
                    match number {
                        Number::Float => "expected a float literal, optionally negated",
                        Number::Integer => "expected an integer literal, optionally negated",
                    },
                ));
            }
        };
        if let Value::Float(number) = value
            && !number.is_finite()
        {
            return Err(Error::new(expr.span(), "a ranged bound must be finite"));
        }
        Ok(Self {
            tokens: quote! { #expr },
            value,
        })
    }
}

#[derive(Default)]
struct Spec {
    min: Option<Bound>,
    max: Option<Bound>,
    default: Option<Bound>,
    clamp: bool,
}

impl Spec {
    fn read(input: &DeriveInput, number: Number) -> Result<Self> {
        let mut attributes = input
            .attrs
            .iter()
            .filter(|attribute| attribute.path().is_ident("ranged"));
        let attribute = attributes.next().ok_or_else(|| {
            Error::new(
                input.ident.span(),
                "Ranged needs `#[ranged(min = .., max = ..)]`",
            )
        })?;
        if let Some(repeated) = attributes.next() {
            return Err(Error::new(repeated.span(), "ranged attribute given twice"));
        }
        let mut spec = Self::default();
        attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("clamp") {
                if spec.clamp {
                    return Err(meta.error("clamp given twice"));
                }
                spec.clamp = true;
                return Ok(());
            }
            let slot = if meta.path.is_ident("min") {
                &mut spec.min
            } else if meta.path.is_ident("max") {
                &mut spec.max
            } else if meta.path.is_ident("default") {
                &mut spec.default
            } else {
                return Err(meta.error("expected min, max, default, or clamp"));
            };
            if slot.is_some() {
                return Err(meta.error("bound given twice"));
            }
            let expr = meta.value()?.parse::<Expr>()?;
            *slot = Some(Bound::read(&expr, number)?);
            Ok(())
        })?;
        Ok(spec)
    }
}

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match derive(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn derive(input: &DeriveInput) -> Result<TokenStream2> {
    if !input.generics.params.is_empty() || input.generics.where_clause.is_some() {
        return Err(Error::new(
            input.generics.span(),
            "Ranged needs a concrete field type",
        ));
    }
    let Data::Struct(data) = &input.data else {
        return Err(Error::new(
            input.span(),
            "Ranged derives on a newtype: one unnamed field",
        ));
    };
    let Fields::Unnamed(fields) = &data.fields else {
        return Err(Error::new(
            data.fields.span(),
            "Ranged derives on a newtype: one unnamed field",
        ));
    };
    if fields.unnamed.len() != 1 {
        return Err(Error::new(
            fields.span(),
            "Ranged derives on a newtype: one unnamed field",
        ));
    }
    let ty = &fields.unnamed[0].ty;
    let (number, primitive) = Number::of(ty).ok_or_else(|| {
        Error::new(
            ty.span(),
            "Ranged holds a primitive number: f32, f64, or an integer",
        )
    })?;
    let spec = Spec::read(input, number)?;
    let min = spec
        .min
        .ok_or_else(|| Error::new(input.span(), "ranged needs min"))?;
    let max = spec
        .max
        .ok_or_else(|| Error::new(input.span(), "ranged needs max"))?;
    if min.value > max.value {
        return Err(Error::new(min.tokens.span(), "min must not exceed max"));
    }
    if let Some(default) = &spec.default
        && !(min.value..=max.value).contains(&default.value)
    {
        return Err(Error::new(
            default.tokens.span(),
            "default must lie inside the range",
        ));
    }
    if spec.clamp && spec.default.is_none() {
        return Err(Error::new(input.span(), "`clamp` needs `default`"));
    }
    let name = &input.ident;
    let label = name.to_string();
    let min = min.tokens;
    let max = max.tokens;
    let finite = (number == Number::Float).then(|| quote! { value.is_finite() && });
    let default_const = spec.default.as_ref().map(|bound| {
        let value = &bound.tokens;
        quote! { pub const DEFAULT: Self = Self(#value); }
    });
    let default_impl = spec.default.as_ref().map(|_| {
        quote! {
            #[automatically_derived]
            impl ::core::default::Default for #name {
                fn default() -> Self { Self::DEFAULT }
            }
        }
    });
    let clamping = spec.clamp.then(|| {
        let nan = (number == Number::Float).then(|| {
            quote! {
                if value.is_nan() { return Self::DEFAULT; }
            }
        });
        quote! {
            #[automatically_derived]
            impl ::core::convert::From<#ty> for #name {
                fn from(value: #ty) -> Self {
                    #nan
                    Self(value.clamp(Self::MIN.0, Self::MAX.0))
                }
            }
        }
    });
    let deserialize = format_ident!(
        "deserialize_{}",
        match primitive.to_string().as_str() {
            "usize" => "u64".to_owned(),
            "isize" => "i64".to_owned(),
            name => name.to_owned(),
        }
    );
    let visits = ["i64", "u64", "i128", "u128", "f64"].map(|source| {
        let source = format_ident!("{source}");
        let visit = format_ident!("visit_{source}");
        quote! {
            fn #visit<E>(self, value: #source) -> ::core::result::Result<Self::Value, E>
            where E: ::serde::de::Error {
                let value = <#ty as ::serde::Deserialize<'de>>::deserialize(
                    ::serde::de::IntoDeserializer::<E>::into_deserializer(value)
                )?;
                #name::checked(value).ok_or_else(|| E::custom(::core::format_args!(
                    "{} must be between {} and {}, got {}", #label, #name::MIN.0, #name::MAX.0, value
                )))
            }
        }
    });
    Ok(quote! {
        #[automatically_derived]
        impl #name {
            pub const MIN: Self = Self(#min);
            pub const MAX: Self = Self(#max);
            #default_const
            #[must_use]
            pub fn checked(value: #ty) -> ::core::option::Option<Self> {
                if #finite (Self::MIN.0..=Self::MAX.0).contains(&value) {
                    ::core::option::Option::Some(Self(value))
                } else {
                    ::core::option::Option::None
                }
            }
        }
        #default_impl
        #clamping
        #[automatically_derived]
        impl ::core::convert::From<#name> for #ty {
            fn from(value: #name) -> Self { value.0 }
        }
        #[automatically_derived]
        impl<'de> ::serde::Deserialize<'de> for #name {
            fn deserialize<D>(deserializer: D) -> ::core::result::Result<Self, D::Error>
            where D: ::serde::Deserializer<'de> {
                struct RangedVisitor;
                #[automatically_derived]
                impl<'de> ::serde::de::Visitor<'de> for RangedVisitor {
                    type Value = #name;
                    fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                        ::core::write!(formatter, "{} between {} and {}", #label, #name::MIN.0, #name::MAX.0)
                    }
                    #(#visits)*
                }
                deserializer.#deserialize(RangedVisitor)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::derive;

    fn expansion(input: syn::DeriveInput) -> String {
        derive(&input).expect("the input is valid").to_string()
    }

    fn refusal(input: syn::DeriveInput) -> String {
        derive(&input)
            .expect_err("the input is invalid")
            .to_string()
    }

    #[test]
    fn a_float_field_is_guarded_against_non_finite_values() {
        let expanded = expansion(parse_quote! {
            #[ranged(min = -24.0, max = 6.0, default = 0.0, clamp)]
            struct Probe(f32);
        });

        assert!(
            expanded.contains("is_finite"),
            "checked rejects an infinity: {expanded}"
        );
        assert!(
            expanded.contains("is_nan"),
            "the clamping From maps NaN to DEFAULT: {expanded}"
        );
    }

    #[test]
    fn an_integer_field_carries_no_float_predicate() {
        let expanded = expansion(parse_quote! {
            #[ranged(min = 0, max = 100, default = 100)]
            struct Share(u8);
        });

        assert!(
            !expanded.contains("is_finite"),
            "an integer is always finite: {expanded}"
        );
        assert!(
            !expanded.contains("is_nan"),
            "an integer is never NaN: {expanded}"
        );
    }

    #[test]
    fn without_clamp_no_conversion_into_the_type_exists() {
        let expanded = expansion(parse_quote! {
            #[ranged(min = 0.25, max = 4.0, default = 1.0)]
            struct Scale(f64);
        });

        assert!(
            !expanded.contains("From < f64 > for Scale"),
            "every door refuses without `clamp`: {expanded}"
        );
    }

    #[test]
    fn without_a_default_the_type_has_none() {
        let expanded = expansion(parse_quote! {
            #[ranged(min = 1.0, max = 1_000.0)]
            struct Tempo(f64);
        });

        assert!(
            !expanded.contains("DEFAULT"),
            "no declared default, no const: {expanded}"
        );
        assert!(
            !expanded.contains("Default for Tempo"),
            "and no impl: {expanded}"
        );
    }

    #[test]
    fn a_negated_bound_is_read_as_a_number() {
        let expanded = expansion(parse_quote! {
            #[ranged(min = -24.0, max = 6.0, default = 0.0)]
            struct Probe(f32);
        });

        assert!(
            expanded.contains("- 24.0"),
            "the negation reaches the emitted const: {expanded}"
        );
    }

    #[test]
    fn a_struct_with_named_fields_is_refused() {
        assert!(
            refusal(parse_quote! {
                #[ranged(min = 0, max = 1)]
                struct Named { value: u8 }
            })
            .contains("one unnamed field")
        );
    }

    #[test]
    fn a_struct_with_two_fields_is_refused() {
        assert!(
            refusal(parse_quote! {
                #[ranged(min = 0, max = 1)]
                struct Pair(u8, u8);
            })
            .contains("one unnamed field")
        );
    }

    #[test]
    fn a_generic_type_is_refused() {
        assert!(
            refusal(parse_quote! {
                #[ranged(min = 0, max = 1)]
                struct Generic<T>(T);
            })
            .contains("concrete field type")
        );
    }

    #[test]
    fn a_non_primitive_field_is_refused() {
        assert!(
            refusal(parse_quote! {
                #[ranged(min = 0, max = 1)]
                struct Wrapped(String);
            })
            .contains("primitive number")
        );
    }

    #[test]
    fn a_missing_attribute_is_refused() {
        assert!(refusal(parse_quote! { struct Bare(u8); }).contains("needs `#[ranged("));
    }

    #[test]
    fn an_unknown_key_is_refused() {
        assert!(
            refusal(parse_quote! {
                #[ranged(min = 0, max = 1, rename = "x")]
                struct Odd(u8);
            })
            .contains("expected")
        );
    }

    #[test]
    fn a_repeated_key_is_refused() {
        assert!(
            refusal(parse_quote! {
                #[ranged(min = 0, min = 1, max = 2)]
                struct Twice(u8);
            })
            .contains("given twice")
        );
    }

    #[test]
    fn a_float_literal_on_an_integer_field_is_refused() {
        assert!(
            refusal(parse_quote! {
                #[ranged(min = 0.0, max = 100.0)]
                struct Share(u8);
            })
            .contains("integer literal")
        );
    }

    #[test]
    fn a_minimum_above_the_maximum_is_refused() {
        assert!(
            refusal(parse_quote! {
                #[ranged(min = 10, max = 1)]
                struct Inverted(u8);
            })
            .contains("must not exceed")
        );
    }

    #[test]
    fn integer_ordering_preserves_adjacent_large_bounds() {
        for input in [
            parse_quote! { #[ranged(min = 9007199254740993, max = 9007199254740992)] struct Large(u64); },
            parse_quote! { #[ranged(min = -9007199254740992, max = -9007199254740993)] struct Negative(i64); },
        ] {
            assert!(refusal(input).contains("must not exceed"));
        }
        expansion(parse_quote! {
            #[ranged(min = 0, max = 340282366920938463463374607431768211455)]
            struct Full(u128);
        });
    }

    #[test]
    fn a_default_outside_the_range_is_refused() {
        assert!(
            refusal(parse_quote! {
                #[ranged(min = 0, max = 10, default = 11)]
                struct Outside(u8);
            })
            .contains("must lie inside")
        );
    }

    #[test]
    fn clamp_without_a_default_is_refused() {
        assert!(
            refusal(parse_quote! {
                #[ranged(min = 0.0, max = 1.0, clamp)]
                struct Homeless(f32);
            })
            .contains("`clamp` needs `default`")
        );
    }
}
