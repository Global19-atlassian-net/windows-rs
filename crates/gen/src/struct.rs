use crate::*;
use squote::{format_ident, quote, Literal, TokenStream};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub struct Struct {
    pub name: TypeName,
    pub fields: Vec<(String, Type)>,
    pub constants: Vec<(String, ConstantValue)>,
    pub signature: String,
    pub is_typedef: bool,
    pub guid: TypeGuid,
    pub nested: BTreeMap<&'static str, Self>,
}

impl Struct {
    pub fn from_type_name(name: TypeName) -> Self {
        let is_winrt = name.def.is_winrt();

        let signature = if is_winrt {
            name.struct_signature()
        } else {
            String::new()
        };

        let mut nested = BTreeMap::new();

        // TODO: push this into the TypeReader, so I get back an iterator of TypeDefs
        if let Some(nested_types) = name.def.reader.nested_types.get(&name.def.row) {
            for def in nested_types {
                let def = winmd::TypeDef {
                    reader: name.def.reader,
                    row: *def,
                };
                let mut def_name = TypeName::new(&def, Vec::new(), &name.namespace);
                def_name.namespace = name.namespace;

                // TODO: if the metadata name is not generated then perhaps just append the name
                debug_assert!(def_name.name.starts_with("_"));

                def_name.name = format!("{}_{}", name.name, nested.len());
                nested.insert(def.name().1, Self::from_type_name(def_name));
            }
        }

        let mut fields = Vec::new();
        let mut constants = Vec::new();
        let mut unique = BTreeSet::new();

        for field in name.def.fields() {
            if field.flags().literal() {
                if let Some(constant) = field.constant() {
                    constants.push((field.name().to_string(), ConstantValue::new(&constant)))
                }
            } else {
                let mut t = Type::from_field(&field, &name.namespace, &nested);

                // TODO: workaround for https://github.com/microsoft/win32metadata/issues/132
                if let TypeKind::Delegate(_) = &t.kind {
                    t.pointers = 0;
                }

                let mut field_name = to_snake(field.name());

                // A handful of Win32 structs, like `CIECHROMA` and `GenTspecParms`, have fields whose snake case
                // names are identical, so we handle this edge case by ensuring they get unique names.
                if !unique.insert(field_name.clone()) {
                    let mut unique_count = 1;

                    loop {
                        unique_count += 1;

                        let unique_field_name = format!("{}{}", field_name, unique_count);

                        if unique.insert(unique_field_name.clone()) {
                            field_name = unique_field_name;
                            break;
                        }
                    }
                }

                fields.push((field_name, t));
            }
        }

        let guid = TypeGuid::from_type_def(&name.def);

        // The C/C++ ABI assumes an empty struct occupies a single byte in memory.
        if fields.is_empty() && guid == TypeGuid::default() {
            let t = Type {
                kind: TypeKind::U8,
                pointers: 0,
                array: None,
                by_ref: false,
                modifiers: Vec::new(),
                param: None,
                name: "".to_string(),
                is_const: false,
                is_array: false,
                is_input: false,
            };

            fields.push(("reserved".to_string(), t));
        }

        let is_typedef = name
            .def
            .has_attribute(("Windows.Win32.Interop", "NativeTypedefAttribute"));

        Self {
            name,
            fields,
            constants,
            signature,
            is_typedef,
            guid,
            nested,
        }
    }

    pub fn dependencies(&self) -> Vec<winmd::TypeDef> {
        self.fields
            .iter()
            .flat_map(|i| i.1.kind.dependencies())
            .chain(
                self.nested
                    .values()
                    .flat_map(|nested| nested.dependencies()),
            )
            .collect()
    }

    pub fn gen(&self) -> TokenStream {
        let name = self.name.gen();

        if self.guid != TypeGuid::default() {
            let guid = self.name.gen_guid(&self.guid);

            return quote! {
                pub const #name: ::windows::Guid = #guid;
            };
        }

        // TODO: if the struct is blittable then don't generate a separate abi type.
        let abi_ident = format_ident!("{}_abi", self.name.name);

        let body = if self.is_typedef {
            let fields = self.fields.iter().map(|(_, kind)| {
                let kind = kind.gen_field();
                quote! {
                    pub #kind
                }
            });

            quote! {
                ( #(#fields),* );
            }
        } else {
            let fields = self.fields.iter().map(|(name, kind)| {
                let name = format_ident(&name);
                let kind = kind.gen_field();
                quote! {
                    pub #name: #kind
                }
            });

            quote! {
                { #(#fields),* }
            }
        };

        let defaults = if self.is_typedef {
            let defaults = self.fields.iter().map(|(_, kind)| {
                let value = kind.gen_default();
                quote! {
                    #value
                }
            });

            quote! {
                Self( #(#defaults),* )
            }
        } else {
            let defaults = self.fields.iter().map(|(name, kind)| {
                let name = format_ident(&name);
                let value = kind.gen_default();
                quote! {
                    #name: #value
                }
            });

            quote! {
                Self{ #(#defaults),* }
            }
        };

        let debug_fields = self
            .fields
            .iter()
            .enumerate()
            .filter_map(|(index, (name, t))| {
                if let TypeKind::Delegate(name) = &t.kind {
                    if !name.def.is_winrt() {
                        return None;
                    }
                }

                if self.is_typedef {
                    let index = Literal::u32_unsuffixed(index as u32);

                    Some(quote! {
                        .field(#name, &format_args!("{:?}", self.#index))
                    })
                } else {
                    let name_ident = format_ident(&name);

                    Some(quote! {
                        .field(#name, &format_args!("{:?}", self.#name_ident))
                    })
                }
            });

        let constants = self.constants.iter().map(|(name, value)| {
            let name = format_ident(&name);
            let value = value.gen();

            quote! {
                pub const #name: #value;
            }
        });

        let compare_fields = if self.fields.is_empty() {
            quote! { true }
        } else {
            let fields = self.fields.iter().enumerate().map(|(index, (name, t))| {
                let name_ident = format_ident(&name);

                if let TypeKind::Delegate(name) = &t.kind {
                    if !name.def.is_winrt() {
                        return quote! {
                            self.#name_ident.map(|f| f as usize) == other.#name_ident.map(|f| f as usize)
                        };
                    }
                }

                if self.is_typedef {
                    let index = Literal::u32_unsuffixed(index as u32);

                    quote! {
                        self.#index == other.#index
                    }
                } else {
                    quote! {
                        self.#name_ident == other.#name_ident
                    }
                }
            });

            quote! {
                #(#fields)&&*
            }
        };

        let abi = self.fields.iter().map(|field| field.1.gen_abi());

        let runtime_type = if self.signature.is_empty() {
            TokenStream::new()
        } else {
            let signature = Literal::byte_string(&self.signature.as_bytes());

            quote! {
                unsafe impl ::windows::RuntimeType for #name {
                    type DefaultType = Self;
                    const SIGNATURE: ::windows::ConstBuffer = ::windows::ConstBuffer::from_slice(#signature);
                }
            }
        };

        // TODO: if blittable then avoid creating a separate ABI struct

         let copy = if self.fields.iter().all(|field| field.1.kind.is_blittable()) {
             quote! {
                 impl ::std::marker::Copy for #name {}
             }
         } else {
             quote! {}
         };

        let debug_name = &self.name.name;

        let nested = self.nested.values().map(|nested| nested.gen());

        if self.name.def.flags().explicit() {
            quote! {
                #[repr(C)]
                #[allow(non_snake_case)]
                #[derive( ::std::clone::Clone)]
                pub union #name #body
                #(#nested)*
                #copy
            }
        } else {
            if self
                .nested
                .values()
                .any(|nested| nested.name.def.flags().explicit())
            {
                quote! {
                    #[repr(C)]
                    #[allow(non_snake_case)]
                    #[derive( ::std::clone::Clone)]
                    pub struct #name #body
                    impl #name {
                        #(#constants)*
                    }
                    #(#nested)*
                    #copy
                }
            } else {
                quote! {
                    #[repr(C)]
                    #[allow(non_snake_case)]
                    #[derive( ::std::clone::Clone)]
                    pub struct #name #body
                    #[repr(C)]
                    #[doc(hidden)]
                    pub struct #abi_ident(#(#abi),*);
                    impl #name {
                        #(#constants)*
                    }
                    unsafe impl ::windows::Abi for #name {
                        type Abi = #abi_ident;
                    }
                    impl ::std::default::Default for #name {
                        fn default() -> Self {
                            #defaults
                        }
                    }
                    impl ::std::fmt::Debug for #name {
                        fn fmt(&self, fmt: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                            fmt.debug_struct(#debug_name)
                                #(#debug_fields)*
                                .finish()
                        }
                    }
                    impl ::std::cmp::PartialEq for #name {
                        fn eq(&self, other: &Self) -> bool {
                            #compare_fields
                        }
                    }
                    impl ::std::cmp::Eq for #name {}
                    #copy
                    #runtime_type
                    #(#nested)*
                }
            }
        }
    }
}
