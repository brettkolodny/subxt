// Copyright 2019-2022 Parity Technologies (UK) Ltd.
// This file is part of subxt.
//
// subxt is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// subxt is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with subxt.  If not, see <http://www.gnu.org/licenses/>.

use super::{
    Field,
    GeneratedTypeDerives,
    TypeDefParameters,
    TypeGenerator,
    TypeParameter,
    TypePath,
};
use heck::CamelCase as _;
use proc_macro2::TokenStream;
use proc_macro_error::abort_call_site;
use quote::{
    format_ident,
    quote,
};
use scale_info::{
    TypeDef,
    TypeDefPrimitive,
};

/// Representation of a type which consists of a set of fields. Used to generate Rust code for
/// either a standalone `struct` definition, or an `enum` variant.
///
/// Fields can either be named or unnamed in either case.
#[derive(Debug)]
pub struct CompositeDef {
    /// The name of the `struct`, or the name of the `enum` variant.
    pub name: syn::Ident,
    /// Generate either a standalone `struct` or an `enum` variant.
    pub kind: CompositeDefKind,
    /// The fields of the type, which are either all named or all unnamed.
    pub fields: CompositeDefFields,
}

impl CompositeDef {
    /// Construct a definition which will generate code for a standalone `struct`.
    pub fn struct_def(
        ident: &str,
        type_params: TypeDefParameters,
        fields_def: CompositeDefFields,
        field_visibility: Option<syn::Visibility>,
        type_gen: &TypeGenerator,
    ) -> Self {
        let mut derives = type_gen.derives().clone();
        let fields: Vec<_> = fields_def.field_types().collect();

        if fields.len() == 1 {
            // any single field wrapper struct with a concrete unsigned int type can derive
            // CompactAs.
            let field = &fields[0];
            if !type_params
                .params()
                .iter()
                .any(|tp| Some(tp.original_name.to_string()) == field.type_name)
            {
                let ty = type_gen.resolve_type(field.type_id);
                if matches!(
                    ty.type_def(),
                    TypeDef::Primitive(
                        TypeDefPrimitive::U8
                            | TypeDefPrimitive::U16
                            | TypeDefPrimitive::U32
                            | TypeDefPrimitive::U64
                            | TypeDefPrimitive::U128
                    )
                ) {
                    derives.push_codec_compact_as()
                }
            }
        }

        let name = format_ident!("{}", ident.to_camel_case());

        Self {
            name,
            kind: CompositeDefKind::Struct {
                derives,
                type_params,
                field_visibility,
            },
            fields: fields_def,
        }
    }

    /// Construct a definition which will generate code for an `enum` variant.
    pub fn enum_variant_def(ident: &str, fields: CompositeDefFields) -> Self {
        let name = format_ident!("{}", ident);
        Self {
            name,
            kind: CompositeDefKind::EnumVariant,
            fields,
        }
    }
}

impl quote::ToTokens for CompositeDef {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let name = &self.name;

        let decl = match &self.kind {
            CompositeDefKind::Struct {
                derives,
                type_params,
                field_visibility,
            } => {
                let phantom_data = type_params.unused_params_phantom_data();
                let fields = self
                    .fields
                    .to_struct_field_tokens(phantom_data, field_visibility.as_ref());
                let trailing_semicolon = matches!(
                    self.fields,
                    CompositeDefFields::NoFields | CompositeDefFields::Unnamed(_)
                )
                .then(|| quote!(;));

                quote! {
                    #derives
                    pub struct #name #type_params #fields #trailing_semicolon
                }
            }
            CompositeDefKind::EnumVariant => {
                let fields = self.fields.to_enum_variant_field_tokens();

                quote! {
                    #name #fields
                }
            }
        };
        tokens.extend(decl)
    }
}

/// Which kind of composite type are we generating, either a standalone `struct` or an `enum`
/// variant.
#[derive(Debug)]
pub enum CompositeDefKind {
    /// Composite type comprising a Rust `struct`.
    Struct {
        derives: GeneratedTypeDerives,
        type_params: TypeDefParameters,
        field_visibility: Option<syn::Visibility>,
    },
    /// Comprises a variant of a Rust `enum`.
    EnumVariant,
}

/// Encapsulates the composite fields, keeping the invariant that all fields are either named or
/// unnamed.
#[derive(Debug)]
pub enum CompositeDefFields {
    NoFields,
    Named(Vec<(syn::Ident, CompositeDefFieldType)>),
    Unnamed(Vec<CompositeDefFieldType>),
}

impl CompositeDefFields {
    /// Construct a new set of composite fields from the supplied [`::scale_info::Field`]s.
    pub fn from_scale_info_fields(
        name: &str,
        fields: &[Field],
        parent_type_params: &[TypeParameter],
        type_gen: &TypeGenerator,
    ) -> Self {
        if fields.is_empty() {
            return Self::NoFields
        }

        let mut named_fields = Vec::new();
        let mut unnamed_fields = Vec::new();

        for field in fields {
            let type_path =
                type_gen.resolve_type_path(field.ty().id(), parent_type_params);
            let field_type = CompositeDefFieldType::new(
                field.ty().id(),
                type_path,
                field.type_name().cloned(),
            );

            if let Some(name) = field.name() {
                let field_name = format_ident!("{}", name);
                named_fields.push((field_name, field_type))
            } else {
                unnamed_fields.push(field_type)
            }
        }

        if !named_fields.is_empty() && !unnamed_fields.is_empty() {
            abort_call_site!(
                "'{}': Fields should either be all named or all unnamed.",
                name,
            )
        }

        if !named_fields.is_empty() {
            Self::Named(named_fields)
        } else {
            Self::Unnamed(unnamed_fields)
        }
    }

    /// Returns the set of composite fields.
    pub fn field_types(&self) -> Box<dyn Iterator<Item = &CompositeDefFieldType> + '_> {
        match self {
            Self::NoFields => Box::new([].iter()),
            Self::Named(named_fields) => Box::new(named_fields.iter().map(|(_, f)| f)),
            Self::Unnamed(unnamed_fields) => Box::new(unnamed_fields.iter()),
        }
    }

    /// Generate the code for fields which will compose a `struct`.
    pub fn to_struct_field_tokens(
        &self,
        phantom_data: Option<syn::TypePath>,
        visibility: Option<&syn::Visibility>,
    ) -> TokenStream {
        match self {
            Self::NoFields => {
                if let Some(phantom_data) = phantom_data {
                    quote! { ( #phantom_data ) }
                } else {
                    quote! {}
                }
            }
            Self::Named(ref fields) => {
                let fields = fields.iter().map(|(name, ty)| {
                    let compact_attr = ty.compact_attr();
                    quote! { #compact_attr #visibility #name: #ty }
                });
                let marker = phantom_data.map(|phantom_data| {
                    quote!(
                        #[codec(skip)]
                        #visibility __subxt_unused_type_params: #phantom_data
                    )
                });
                quote!(
                    {
                        #( #fields, )*
                        #marker
                    }
                )
            }
            Self::Unnamed(ref fields) => {
                let fields = fields.iter().map(|ty| {
                    let compact_attr = ty.compact_attr();
                    quote! { #compact_attr #visibility #ty }
                });
                let marker = phantom_data.map(|phantom_data| {
                    quote!(
                        #[codec(skip)]
                        #visibility #phantom_data
                    )
                });
                quote! {
                    (
                        #( #fields, )*
                        #marker
                    )
                }
            }
        }
    }

    /// Generate the code for fields which will compose an `enum` variant.
    pub fn to_enum_variant_field_tokens(&self) -> TokenStream {
        match self {
            Self::NoFields => quote! {},
            Self::Named(ref fields) => {
                let fields = fields.iter().map(|(name, ty)| {
                    let compact_attr = ty.compact_attr();
                    quote! { #compact_attr #name: #ty }
                });
                quote!( { #( #fields, )* } )
            }
            Self::Unnamed(ref fields) => {
                let fields = fields.iter().map(|ty| {
                    let compact_attr = ty.compact_attr();
                    quote! { #compact_attr #ty }
                });
                quote! { ( #( #fields, )* ) }
            }
        }
    }
}

/// Represents a field of a composite type to be generated.
#[derive(Debug)]
pub struct CompositeDefFieldType {
    pub type_id: u32,
    pub type_path: TypePath,
    pub type_name: Option<String>,
}

impl CompositeDefFieldType {
    /// Construct a new [`CompositeDefFieldType`].
    pub fn new(type_id: u32, type_path: TypePath, type_name: Option<String>) -> Self {
        CompositeDefFieldType {
            type_id,
            type_path,
            type_name,
        }
    }

    /// Returns `true` if the field is a [`::std::boxed::Box`].
    pub fn is_boxed(&self) -> bool {
        // Use the type name to detect a `Box` field.
        // Should be updated once `Box` types are no longer erased:
        // https://github.com/paritytech/scale-info/pull/82
        matches!(&self.type_name, Some(ty_name) if ty_name.contains("Box<"))
    }

    /// Returns the `#[codec(compact)]` attribute if the type is compact.
    fn compact_attr(&self) -> Option<TokenStream> {
        self.type_path
            .is_compact()
            .then(|| quote!( #[codec(compact)] ))
    }
}

impl quote::ToTokens for CompositeDefFieldType {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ty_path = &self.type_path;

        if self.is_boxed() {
            tokens.extend(quote! { ::std::boxed::Box<#ty_path> })
        } else {
            tokens.extend(quote! { #ty_path })
        };
    }
}
