//! This module contains the Lola standard library.

use crate::ty::{Ty, TypeConstraint};

#[derive(Debug)]
pub struct Generic {
    pub constraint: TypeConstraint,
}

#[derive(Debug)]
pub enum Parameter {
    Type(Ty),
    /// Index into associated generics array
    Generic(u8),
}

/// different kinds of type declarations, can be e.g., alias, newtype, struct, enum
#[derive(Debug)]
pub enum TypeDecl {
    //Alias(String, Ty),
//NewType(String, Ty),
//Struct(String, Vec<(String, Ty)>),
}

/// A (possibly generic) function declaration
#[derive(Debug)]
pub struct FuncDecl {
    pub name: String,
    pub generics: Vec<Generic>,
    pub parameters: Vec<Parameter>,
    pub return_type: Parameter,
}

use crate::analysis::naming::{Declaration, ScopedDecl};

lazy_static! {
    // fn sqrt<T: FloatingPoint>(T) -> T
    static ref SQRT: FuncDecl = FuncDecl {
        name: "sqrt".to_string(),
        generics: vec![Generic {
            constraint: TypeConstraint::FloatingPoint,
        }],
        parameters: vec![Parameter::Generic(0)],
        return_type: Parameter::Generic(0),
    };
    // fn cos<T: FloatingPoint>(T) -> T
    static ref COS: FuncDecl = FuncDecl {
        name: "cos".to_string(),
        generics: vec![Generic {
            constraint: TypeConstraint::FloatingPoint,
        }],
        parameters: vec![Parameter::Generic(0)],
        return_type: Parameter::Generic(0),
    };
    // fn sin<T: FloatingPoint>(T) -> T
    static ref SIN: FuncDecl = FuncDecl {
        name: "sin".to_string(),
        generics: vec![Generic {
            constraint: TypeConstraint::FloatingPoint,
        }],
        parameters: vec![Parameter::Generic(0)],
        return_type: Parameter::Generic(0),
    };
}

pub(crate) fn import_math_module<'a>(scope: &mut ScopedDecl<'a>) {
    scope.add_decl_for(&SQRT.name, Declaration::Func(&SQRT));
    scope.add_decl_for(&COS.name, Declaration::Func(&COS));
    scope.add_decl_for(&SIN.name, Declaration::Func(&SIN));
}

pub(crate) struct MethodLookup {}

impl MethodLookup {
    pub(crate) fn new() -> MethodLookup {
        MethodLookup {}
    }

    pub(crate) fn get(&self, ty: &Ty, name: &str) -> Option<FuncDecl> {
        match (ty, name) {
            (Ty::EventStream(inner), "offset") => Some(FuncDecl {
                name: "offset".to_string(),
                generics: vec![Generic {
                    constraint: TypeConstraint::Integer,
                }],
                parameters: vec![Parameter::Type(ty.clone()), Parameter::Generic(0)],
                return_type: Parameter::Type(Ty::Option(inner.clone())),
            }),
            (Ty::EventStream(inner), "window") => Some(FuncDecl {
                name: "window".to_string(),
                generics: vec![Generic {
                    constraint: TypeConstraint::Duration,
                }],
                parameters: vec![Parameter::Type(ty.clone())],
                return_type: Parameter::Type(Ty::Option(inner.clone())),  // TODO: return type is wrong
            }),
            (Ty::Option(inner), "default") => Some(FuncDecl {
                name: "default".to_string(),
                generics: Vec::new(),
                parameters: vec![
                    Parameter::Type(ty.clone()),
                    Parameter::Type((**inner).clone()),
                ],
                return_type: Parameter::Type((**inner).clone()),
            }),
            _ => unimplemented!("{} for {}", name, ty),
        }
    }
}
